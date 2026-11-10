#!/usr/bin/env bash
# Register a GitHub Actions self-hosted runner on a gpubench box, after
# provision-cuda.yml or provision-metal.yml has run. Labels:
# self-hosted,gpubench,<box>, which gpubench-run.yml's runs-on names. The box
# label is <kind>-<class>, e.g. cuda-l4 (.github/scripts/gpubench-boxes.mjs);
# name the hardware class, so a box torn down and brought back on the same
# hardware keeps its history.
#
#   cuda, as root:
#     sudo GPUBENCH_RUNNER_TOKEN=<TOKEN> bash scripts/gpubench/provision/install-runner.sh \
#       --box cuda-l4 [--cuda-arch 89] [--repo wasmCloud/wasmCloud] [--name <name>]
#
#   metal, as the gpubench user, from a Terminal in its logged-in desktop
#   session (the runner is a LaunchAgent there; over SSH it has no session):
#     GPUBENCH_RUNNER_TOKEN=<TOKEN> bash scripts/gpubench/provision/install-runner.sh --box metal-m4pro
#
#   teardown, before the box goes away (same user as the install):
#     GPUBENCH_RUNNER_TOKEN=<REMOVE TOKEN> bash scripts/gpubench/provision/install-runner.sh --remove
#
# Tokens are one-shot: `gh api -X POST repos/wasmCloud/wasmCloud/actions/runners/registration-token`
# (or remove-token), or Settings > Actions > Runners. They're read from the
# environment so they never appear in a process's argv. A box that's already
# gone can be deregistered with `gh api -X DELETE repos/wasmCloud/wasmCloud/actions/runners/<id>`.
#
# The runner's .path and .env set what every job sees: rustup's cargo, the
# CUDA toolkit or Homebrew ahead of the system's tools, and on cuda
# GPUBENCH_CUDA_ARCH, which gpubench-preflight.mjs checks against the GPU.
#
# Bails if the runner directory exists; remove it to re-register.
#
# Untested: neither box exists yet.

set -euo pipefail

REPO="wasmCloud/wasmCloud"
RUNNER_VERSION="2.337.0"
# From https://github.com/actions/runner/releases/tag/v${RUNNER_VERSION}; bump
# with RUNNER_VERSION. Same version and linux-x64 checksum as
# scripts/bench/install-runner.sh.
RUNNER_SHA256_LINUX_X64="70920811a4f8ad4328818682bca5c6469c1c942fab52448868071d0063816613"
RUNNER_SHA256_OSX_ARM64="5a2cd92908a93d7276a194e1de6008099f3e7946f3f8e14aa7a1a7b4a31fdec2"
RUNNER_USER="gpubench"
# What gpubench-preflight.mjs compares WASMCLOUD_BENCH_<LABEL>_HOSTNAME with. Short-
# lived boxes get a fixed hostname (e.g. from cloud-init), so it stays put.
host="$(hostname | sed 's/\.local$//')"
RUNNER_NAME="$host"
CUDA_ARCH=""
BOX=""
REMOVE=no

usage() {
  sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --box) BOX="$2"; shift 2 ;;
    --remove) REMOVE=yes; shift ;;
    --name) RUNNER_NAME="$2"; shift 2 ;;
    --cuda-arch) CUDA_ARCH="$2"; shift 2 ;;
    -h | --help) usage 0 ;;
    *) echo "unknown arg: $1" >&2; usage 2 ;;
  esac
done

[ -n "${GPUBENCH_RUNNER_TOKEN:-}" ] || { echo "GPUBENCH_RUNNER_TOKEN env var required" >&2; exit 1; }

step() { printf '\n=== %s ===\n' "$*"; }

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    kind=cuda
    [ "$(id -u)" -eq 0 ] || { echo "cuda: run as root" >&2; exit 1; }
    id "$RUNNER_USER" >/dev/null 2>&1 || { echo "no $RUNNER_USER user; run provision-cuda.yml first" >&2; exit 1; }
    home="$(getent passwd "$RUNNER_USER" | cut -d: -f6)"
    runner_dir=/opt/actions-runner
    work_dir="$home/work"
    tarball_os=linux-x64
    sha256="$RUNNER_SHA256_LINUX_X64"
    as_runner() { sudo -u "$RUNNER_USER" -H "$@"; }
    if [ "$REMOVE" = no ] && [ -z "$CUDA_ARCH" ]; then
      CUDA_ARCH="$(nvidia-smi -i "${GPUBENCH_GPU:-0}" --query-gpu=compute_cap --format=csv,noheader | tr -d '. ')"
    fi
    [ "$REMOVE" = yes ] || [[ "$CUDA_ARCH" =~ ^[0-9]+$ ]] || { echo "could not tell the GPU's CUDA arch; pass --cuda-arch" >&2; exit 1; }
    path="$home/.cargo/bin:/usr/local/cuda/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    ;;
  Darwin-arm64)
    kind=metal
    [ "$(id -un)" = "$RUNNER_USER" ] || { echo "metal: run as $RUNNER_USER, not $(id -un)" >&2; exit 1; }
    launchctl print "gui/$(id -u)" >/dev/null 2>&1 ||
      { echo "metal: no desktop session for $RUNNER_USER; run this from its logged-in Terminal" >&2; exit 1; }
    home="$HOME"
    runner_dir="$home/actions-runner"
    work_dir="$home/work"
    tarball_os=osx-arm64
    sha256="$RUNNER_SHA256_OSX_ARM64"
    as_runner() { "$@"; }
    path="$home/.cargo/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    ;;
  *) echo "no gpubench box is $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

if [ "$REMOVE" = yes ]; then
  [ -d "$runner_dir" ] || { echo "no runner at $runner_dir" >&2; exit 1; }
  step "stop + uninstall the runner service"
  cd "$runner_dir"
  ./svc.sh stop || true
  ./svc.sh uninstall
  step "deregister the runner"
  export GPUBENCH_RUNNER_TOKEN
  # shellcheck disable=SC2016  # expanded by the inner shell
  remove='cd "$1" && ./config.sh remove --token "$GPUBENCH_RUNNER_TOKEN"'
  if [ "$kind" = cuda ]; then
    sudo --preserve-env=GPUBENCH_RUNNER_TOKEN -u "$RUNNER_USER" bash -c "$remove" _ "$runner_dir"
  else
    bash -c "$remove" _ "$runner_dir"
  fi
  cd /
  rm -rf "$runner_dir"
  echo "Runner removed. Keep the repo variable and the published rows: the next box with the same label reuses both."
  exit 0
fi

[[ "$BOX" =~ ^${kind}-[a-z0-9]+(-[a-z0-9]+)*$ ]] ||
  { echo "--box: expected ${kind}-<class> on this machine, e.g. ${kind}-l4 (got '${BOX}')" >&2; exit 1; }
box="$BOX"
host_var="WASMCLOUD_BENCH_$(echo "$box" | tr 'a-z-' 'A-Z_')_HOSTNAME"
labels="self-hosted,gpubench,$box"

[ ! -d "$runner_dir" ] || { echo "$runner_dir already exists; remove it and re-run with a fresh token" >&2; exit 1; }
[ -x "$home/.cargo/bin/cargo" ] || { echo "no cargo for $RUNNER_USER; run provision-$kind.yml first" >&2; exit 1; }

step "download + verify + extract actions-runner v${RUNNER_VERSION} ($tarball_os)"
mkdir -p "$runner_dir" "$work_dir"
[ "$kind" = metal ] || chown "$RUNNER_USER:$RUNNER_USER" "$runner_dir" "$work_dir"
tarball="$runner_dir/runner.tgz"
url="https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-${tarball_os}-${RUNNER_VERSION}.tar.gz"
as_runner curl -fL -o "$tarball" "$url"
got="$(shasum -a 256 "$tarball" 2>/dev/null || sha256sum "$tarball")"
[ "${got%% *}" = "$sha256" ] || { echo "runner tarball sha256 ${got%% *}, expected $sha256" >&2; exit 1; }
as_runner tar -xzf "$tarball" -C "$runner_dir"
rm -f "$tarball"

step "configure runner (registers with GitHub)"
export GPUBENCH_RUNNER_TOKEN GPUBENCH_REPO="$REPO" GPUBENCH_NAME="$RUNNER_NAME" \
  GPUBENCH_LABELS="$labels" GPUBENCH_WORK="$work_dir" GPUBENCH_RUNNER_DIR="$runner_dir"
# shellcheck disable=SC2016  # expanded by the inner shell
config='cd "$GPUBENCH_RUNNER_DIR" && ./config.sh --unattended --replace \
  --url "https://github.com/${GPUBENCH_REPO}" --token "${GPUBENCH_RUNNER_TOKEN}" \
  --name "${GPUBENCH_NAME}" --labels "${GPUBENCH_LABELS}" --work "${GPUBENCH_WORK}"'
if [ "$kind" = cuda ]; then
  sudo --preserve-env=GPUBENCH_RUNNER_TOKEN,GPUBENCH_REPO,GPUBENCH_NAME,GPUBENCH_LABELS,GPUBENCH_WORK,GPUBENCH_RUNNER_DIR \
    -u "$RUNNER_USER" bash -c "$config"
else
  bash -c "$config"
fi

step "job environment (.path, .env)"
as_runner sh -c "printf '%s\n' '$path' >'$runner_dir/.path'"
{
  echo "GPUBENCH_BOX=$box"
  [ "$kind" = cuda ] && echo "GPUBENCH_CUDA_ARCH=$CUDA_ARCH"
  true
} | as_runner tee -a "$runner_dir/.env" >/dev/null

step "install + start the runner service"
cd "$runner_dir"
if [ "$kind" = cuda ]; then
  ./svc.sh install "$RUNNER_USER"
else
  ./svc.sh install
fi
./svc.sh start
./svc.sh status | head -20

cat <<EOM

Runner registered:
  repo:    ${REPO}
  name:    ${RUNNER_NAME}
  labels:  ${labels}
  workdir: ${work_dir}

Set the repo variable ${host_var} to ${host} (once per label: a box brought
back with the same label and hostname needs no change), then run
Actions > gpubench with box=${box} and smoke=true.
EOM
