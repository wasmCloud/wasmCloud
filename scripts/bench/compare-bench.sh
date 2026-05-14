#!/usr/bin/env bash
# Run a single bench against two refs back-to-back on the same host and emit
# a delta table. Used by .github/workflows/bench-compare.yml.
#
#   ./scripts/bench/compare-bench.sh <bench> <ref_a> <ref_b>
#
# Variance handling (per the design call — see scripts/bench/README.md §9.4):
#   - iai_callgrind: 1 run per ref (instruction counts are deterministic).
#   - criterion benches: 3 interleaved runs per ref (a₁ b₁ a₂ b₂ a₃ b₃);
#     median of the three is what the delta is computed from.
#
# Side effects:
#   - Switches the working tree (git checkout). Saves and restores the
#     original HEAD on exit (including failure) so a hung run doesn't
#     leave the repo in a detached state.
#   - Writes per-run snapshots to ${WASMCLOUD_BENCH_COMPARE_DIR:-/tmp/bench-compare}/{a,b}/.
#   - `bench-tools delta` (invoked at the end) writes the rendered
#     markdown delta to both stdout and ${WASMCLOUD_BENCH_COMPARE_DIR}/delta.md.
#     The workflow `cat`s delta.md into ${GITHUB_STEP_SUMMARY}.
#
# Does NOT push to S3 or update history.json — comparison runs are
# ephemeral and don't feed the trends timeline.

set -euo pipefail

bench="${1:?bench name required}"
ref_a="${2:?ref_a required (the baseline, typically 'main')}"
ref_b="${3:?ref_b required (the candidate)}"

: "${CARGO_TARGET_DIR:=/var/lib/bench/target}"
compare_dir="${WASMCLOUD_BENCH_COMPARE_DIR:-/tmp/bench-compare-${GITHUB_RUN_ID:-local}}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Variance regime: instruction-count benches don't benefit from repeats;
# wall-clock benches do. Override via WASMCLOUD_BENCH_COMPARE_ITERS=N for testing.
if [ -n "${WASMCLOUD_BENCH_COMPARE_ITERS:-}" ]; then
  iters="$WASMCLOUD_BENCH_COMPARE_ITERS"
elif [ "$bench" = "iai_callgrind" ]; then
  iters=1
else
  iters=3
fi

# Resolve refs to full SHAs once so the rest of the script logs an unambiguous
# identifier even if a branch tip moves underneath us mid-run.
sha_a=$(git rev-parse "$ref_a")
sha_b=$(git rev-parse "$ref_b")
short_a=$(git rev-parse --short=12 "$sha_a")
short_b=$(git rev-parse --short=12 "$sha_b")
saved_head=$(git rev-parse HEAD)

mkdir -p "${compare_dir}/a" "${compare_dir}/b"

# Build bench-tools BEFORE switching the worktree, then invoke the binary
# at the absolute path cargo reports. If we relied on `cargo run` after
# each checkout we'd pick up whatever version of bench-tools lives at
# that ref — including the no-such-crate case for any ref older than
# this commit. The pinned path keeps the renderer constant across the
# two refs we're comparing.
#
# Discover the path via --message-format=json rather than hardcoding
# ${CARGO_TARGET_DIR}/debug/bench-tools, which is wrong under any of:
# build.target in .cargo/config.toml, custom profile, or a host-triple
# subdir injected by cross-compile toolchains.
#
# Debug build is deliberate. bench-tools does ~zero CPU work (parse a
# few JSON files, render markdown), so the release-mode perf delta is
# invisible next to the ~30 min comparison runs themselves. Debug
# builds ~3× faster than release on this host.
echo "building bench-tools…"
# shellcheck disable=SC1091
. "$HOME/.cargo/env" 2>/dev/null || true
bench_tools=$(
  cargo build -p bench-tools --message-format=json-render-diagnostics \
    | jq -r 'select(.reason == "compiler-artifact"
                    and .target.name == "bench-tools"
                    and (.target.kind | index("bin")))
             | .executable' \
    | grep -v '^null$' \
    | tail -n1
)
if [ -z "$bench_tools" ] || [ ! -x "$bench_tools" ]; then
  echo "bench-tools binary not found after cargo build (got: '$bench_tools')" >&2
  exit 1
fi

# Always return the worktree to where it started — caller (the workflow or
# operator) shouldn't be surprised by detached HEAD on failure. If the
# restore itself fails, log it loudly: the operator needs to know the
# worktree is in an unexpected state, otherwise the next local run picks
# up half-applied state from the comparison.
cleanup() {
  rc=$?
  if ! git checkout --quiet "$saved_head" 2>/dev/null; then
    echo "::warning::failed to restore worktree to ${saved_head:0:12}; worktree may be on a comparison ref" >&2
  fi
  exit $rc
}
trap cleanup EXIT

step() { printf '\n=== %s ===\n' "$*"; }

# Snapshot the bench output dirs that exist (criterion and/or iai) into a
# numbered iteration dir under ${compare_dir}/{a,b}/. Wipes the live dirs
# afterwards so the next iteration starts clean (run-bench.sh also wipes,
# but doing it here guarantees no inter-iteration leak even if run-bench.sh
# changes later).
snapshot() {
  local side="$1" iter="$2"
  local dest="${compare_dir}/${side}/iter-${iter}"
  mkdir -p "$dest"
  for d in criterion iai; do
    if [ -d "${CARGO_TARGET_DIR}/${d}" ]; then
      cp -a "${CARGO_TARGET_DIR}/${d}" "${dest}/${d}"
    fi
  done
}

run_one() {
  local side="$1" sha="$2" iter="$3"
  step "[${side} iter ${iter}] checkout ${sha:0:12} and run ${bench}"
  git checkout --quiet --detach "$sha"
  "${script_dir}/run-bench.sh" "$bench"
  snapshot "$side" "$iter"
}

# Interleave so any thermal / cache drift over the run window splits between
# the two refs rather than landing entirely on one of them.
for i in $(seq 1 "$iters"); do
  run_one a "$sha_a" "$i"
  run_one b "$sha_b" "$i"
done

step "compute delta"
WASMCLOUD_BENCH_COMPARE_DIR="$compare_dir" \
WASMCLOUD_BENCH_SHORT_A="$short_a" WASMCLOUD_BENCH_REF_A="$ref_a" \
WASMCLOUD_BENCH_SHORT_B="$short_b" WASMCLOUD_BENCH_REF_B="$ref_b" \
WASMCLOUD_BENCH_NAME="$bench" WASMCLOUD_BENCH_ITERS="$iters" \
  "$bench_tools" delta
