#!/usr/bin/env bash
# Install a GitHub Actions self-hosted runner on the wasmCloud bench host.
# Run once on the bench host as root.
#
#   sudo WASMCLOUD_BENCH_RUNNER_TOKEN=<TOKEN> bash scripts/bench/install-runner.sh \
#     [--repo  wasmCloud/wasmCloud] \
#     [--version 2.334.0]
#
# The registration token is one-shot. Get one from:
#   GitHub Repo > Settings > Actions > Runners > New self-hosted runner
# (tokens expire after ~1 hour and are consumed by config.sh).
#
# Pass via env (not CLI flag) so the value never appears in /proc/<pid>/cmdline.
#
# Effects:
#   - creates a non-root `bench` user (no shell login by default; runner runs
#     as bench via systemd)
#   - installs AWS CLI v2 + zstd (needed by .github/scripts/bench-push-results.mjs)
#   - installs rustup for the bench user (toolchain auto-installs on first
#     build via the repo's rust-toolchain.toml)
#   - downloads + verifies + extracts actions-runner to /opt/actions-runner
#   - configures the runner with labels: self-hosted,bench,hetzner
#   - registers a systemd service `actions.runner.<owner>-<repo>.bench-host`
#     that starts on boot
#   - creates /var/lib/bench/target with bench: ownership for $CARGO_TARGET_DIR
#
# Idempotency: if the runner directory already exists, the script bails and
# tells you to remove it first. Re-registration requires a fresh token.

set -euo pipefail

REPO="wasmCloud/wasmCloud"
RUNNER_VERSION="2.334.0"
# Default to the actual hostname of the box (set by installimage). Override with
# --name if needed. No hardcoded value here — the box's identity isn't in the repo.
RUNNER_NAME="$(hostname)"
LABELS="self-hosted,bench,hetzner"
RUNNER_DIR="/opt/actions-runner"
WORK_DIR="/var/lib/bench/work"
TARGET_DIR="/var/lib/bench/target"

# SHA256 of actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz, copied from
# https://github.com/actions/runner/releases/tag/v${RUNNER_VERSION}. Bump
# both `RUNNER_VERSION` and this constant together.
RUNNER_SHA256="048024cd2c848eb6f14d5646d56c13a4def2ae7ee3ad12122bee960c56f3d271"

usage() {
  sed -n '2,/^$/p' "$0" | sed 's/^# \?//'
  exit "${1:-0}"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --repo)    REPO="$2"; shift 2 ;;
    --version) RUNNER_VERSION="$2"; shift 2 ;;
    --name)    RUNNER_NAME="$2"; shift 2 ;;
    -h|--help) usage 0 ;;
    *)         echo "unknown arg: $1" >&2; usage 2 ;;
  esac
done

[ "$(id -u)" -eq 0 ]                     || { echo "must run as root"; exit 1; }
[ -n "${WASMCLOUD_BENCH_RUNNER_TOKEN:-}" ]         || { echo "WASMCLOUD_BENCH_RUNNER_TOKEN env var required"; exit 1; }
[ ! -d "$RUNNER_DIR" ]                   || { echo "$RUNNER_DIR already exists; remove + re-run with a fresh token"; exit 1; }

step() { printf '\n=== %s ===\n' "$*"; }

step "create bench user"
if ! id bench >/dev/null 2>&1; then
  useradd --system --create-home --home-dir /var/lib/bench --shell /usr/sbin/nologin bench
fi
mkdir -p "$WORK_DIR" "$TARGET_DIR"
chown -R bench:bench /var/lib/bench

step "install AWS CLI v2 + zstd"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends unzip zstd ca-certificates curl
if ! command -v aws >/dev/null 2>&1 || ! aws --version 2>&1 | grep -q '^aws-cli/2'; then
  tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
  arch=$(uname -m)   # x86_64 → x86_64; aarch64 → aarch64
  curl -fL "https://awscli.amazonaws.com/awscli-exe-linux-${arch}.zip" -o "$tmp/awscliv2.zip"
  unzip -q "$tmp/awscliv2.zip" -d "$tmp"
  "$tmp/aws/install" --update >/dev/null
  trap - EXIT; rm -rf "$tmp"
fi
aws --version

step "install rustup for the bench user"
# The runner runs as `bench`, so it needs its own cargo. Default to `stable`
# so out-of-tree `cargo install` invocations (e.g. gungraun-runner
# below) have something to pick. Inside the wasmCloud workspace the
# repo's rust-toolchain.toml still wins — it also pins `stable` today,
# so there's no drift between the default and the workspace toolchain.
if [ ! -x /var/lib/bench/.cargo/bin/cargo ]; then
  sudo -u bench -H bash -c '
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain stable --no-modify-path
  '
fi
# Idempotent post-condition for the "rustup is installed but had no
# default toolchain" case (earlier versions of this script installed
# with `--default-toolchain none`). `rustup default stable` is a no-op
# when stable is already the default; downloads + sets it otherwise.
sudo -u bench -H bash -c '. $HOME/.cargo/env && rustup default stable && rustup --version'

step "install gungraun-runner for the bench user"
# Required by the gungraun bench. Version must equal the gungraun crate
# version pinned in crates/wash-runtime/Cargo.toml — gungraun enforces
# equality at run time, so they're derived from a single source of truth
# (the Cargo.toml dep) rather than re-pinned here. `provision.yml` does
# the same derivation, so a `cargo update gungraun` bump propagates to
# both install paths without a separate edit.
#
# Version check uses `cargo install --list` rather than
# `gungraun-runner --version`: the runner inspects the nearest Cargo.toml
# at every invocation and bails with "No version information found for
# gungraun" when it's run from the wasmCloud workspace root (the gungraun
# dep lives in crates/wash-runtime/Cargo.toml as a dev-dependency, not in
# [workspace.dependencies]). The install-time check below is
# cwd-independent, so it works whether the operator runs the script from
# /opt/wasmcloud, /tmp, or anywhere else.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cargo_toml="$script_dir/../../crates/wash-runtime/Cargo.toml"
gungraun_version=$(awk -F'"' '/^gungraun = "/ { print $2; exit }' "$cargo_toml")
if [ -z "$gungraun_version" ]; then
  echo "could not extract gungraun version from $cargo_toml" >&2
  echo "expected a line like: gungraun = \"X.Y.Z\"" >&2
  exit 1
fi
echo "installing gungraun-runner v${gungraun_version} (from $cargo_toml)"
sudo --preserve-env=GUNGRAUN_VERSION -u bench -H \
  env GUNGRAUN_VERSION="$gungraun_version" bash -c '
  set -euo pipefail
  . $HOME/.cargo/env
  if ! cargo install --list | grep -qx "gungraun-runner v${GUNGRAUN_VERSION}:"; then
    cargo install gungraun-runner --version "${GUNGRAUN_VERSION}"
  fi
  cargo install --list | grep "^gungraun-runner "
'

step "download + verify + extract actions-runner v${RUNNER_VERSION}"
mkdir -p "$RUNNER_DIR"
chown bench:bench "$RUNNER_DIR"
tarball="$RUNNER_DIR/runner.tgz"
url="https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz"
sudo -u bench bash -c "curl -fL -o '$tarball' '$url'"
echo "${RUNNER_SHA256}  ${tarball}" | sha256sum -c -
sudo -u bench bash -c "cd '$RUNNER_DIR' && tar xzf runner.tgz && rm runner.tgz"

step "configure runner (registers with GitHub)"
# Token via env so it never lands in /proc/<pid>/cmdline. The values are
# already exported in this shell — `sudo --preserve-env=VAR1,VAR2,...`
# carries them through into the target user's environment regardless of
# how the host's sudoers is configured (the `sudo VAR=value cmd` form
# only works when the `setenv` Defaults flag is set, which is true on
# stock Ubuntu but not guaranteed on hardened images).
export WASMCLOUD_BENCH_RUNNER_TOKEN WASMCLOUD_BENCH_REPO="$REPO" \
       WASMCLOUD_BENCH_NAME="$RUNNER_NAME" \
       WASMCLOUD_BENCH_LABELS="$LABELS" \
       WASMCLOUD_BENCH_WORK="$WORK_DIR"
sudo --preserve-env=WASMCLOUD_BENCH_RUNNER_TOKEN,WASMCLOUD_BENCH_REPO,WASMCLOUD_BENCH_NAME,WASMCLOUD_BENCH_LABELS,WASMCLOUD_BENCH_WORK \
     -u bench bash -c '
  cd /opt/actions-runner && ./config.sh \
    --unattended \
    --replace \
    --url "https://github.com/${WASMCLOUD_BENCH_REPO}" \
    --token "${WASMCLOUD_BENCH_RUNNER_TOKEN}" \
    --name "${WASMCLOUD_BENCH_NAME}" \
    --labels "${WASMCLOUD_BENCH_LABELS}" \
    --work "${WASMCLOUD_BENCH_WORK}"
'

step "install + start systemd service"
cd "$RUNNER_DIR"
./svc.sh install bench
./svc.sh start
./svc.sh status | head -20

cat <<EOM

Runner registered:
  repo:    ${REPO}
  name:    ${RUNNER_NAME}
  labels:  ${LABELS}
  workdir: ${WORK_DIR}
  target:  ${TARGET_DIR}

Confirm in GitHub: Settings > Actions > Runners (the new runner should
appear as Idle within a few seconds).

Trigger a run from: Actions > bench > Run workflow.
EOM
