#!/usr/bin/env bash
# Run a single bench from crates/wash-runtime/benches/. Captures the full
# bench output to a log file under $CARGO_TARGET_DIR so bench-push-results.mjs can
# archive it next to the bench data.
#
# Two harness types share this script:
#   - criterion benches (http_invoke, wasmtime_baseline, wasmtime_serve)
#     write to $CARGO_TARGET_DIR/criterion/.
#   - gungraun benches (gungraun) write to $CARGO_TARGET_DIR/gungraun/
#     and are pinned to the isolated CPU (set by hetzner-postinstall.sh
#     via isolcpus=) so that scheduler interference doesn't leak into
#     instruction counts. valgrind serializes threads, so single-core
#     pinning doesn't hurt throughput.
#
# Side effects:
#   - Creates $CARGO_TARGET_DIR/.bench-start-${bench} (mtime marker).
#     `bench-tools jsonl` filters by `-newer` against this marker so
#     stale data from previous runs of *other* benches isn't emitted as
#     the current bench (otherwise the persistent target dir leaks across
#     bench types).

set -euo pipefail

# Bench name validation lives in the workflow_dispatch `choice` input
# (see .github/workflows/bench.yml). cargo bench will surface a clear
# "no bench target named X" error if anything else is passed, so we
# avoid maintaining the bench list in two places.
bench="${1:?bench name required}"

# shellcheck disable=SC1091
. "$HOME/.cargo/env"

: "${CARGO_TARGET_DIR:=/var/lib/bench/target}"
mkdir -p "$CARGO_TARGET_DIR"

# CPU index reserved by isolcpus= in hetzner-postinstall.sh. Must match.
# Override with WASMCLOUD_BENCH_ISOLATED_CPU= for hosts staged differently.
isolated_cpu="${WASMCLOUD_BENCH_ISOLATED_CPU:-5}"

# Marker for this run; `bench-tools jsonl` uses it as the `find -newer`
# reference so stale data from prior runs of other benches isn't picked up.
marker="${CARGO_TARGET_DIR}/.bench-start-${bench}"

log="${CARGO_TARGET_DIR}/run-${bench}-${GITHUB_RUN_ID:-local}.log"
{
  echo "=== run-bench: ${bench} ==="
  echo "host:   $(hostname)"
  echo "kernel: $(uname -srm)"
  echo "cpu:    $(awk -F: '/^model name/{print $2; exit}' /proc/cpuinfo | sed 's/^ //')"
  echo "online: $(nproc) cpu(s)"
  echo "git:    $(git rev-parse HEAD)"
  echo "ref:    ${GITHUB_REF_NAME:-?}"
  echo "ts:     $(date -u +%FT%TZ)"
  echo
} | tee "$log"

# Wipe per-bench measurement dirs before the run. Stale data here would
# otherwise leak across benches because the dir at $CARGO_TARGET_DIR is
# persistent (kept for the *build* cache, not the measurement output).
# The build cache lives elsewhere in $CARGO_TARGET_DIR and is preserved.
# Both criterion and gungraun keep "previous run" baseline data we don't use.
rm -rf "${CARGO_TARGET_DIR}/criterion" "${CARGO_TARGET_DIR}/gungraun"

# Touch the run marker right before invoking cargo so `find -newer "$marker"`
# in `bench-tools jsonl` picks up exactly the files this run produced.
# Belt-and-suspenders with the wipe above: if the wipe is skipped (e.g.,
# manual partial run), the marker still bounds what gets emitted.
touch "$marker"

# Build the wasm fixtures the benches `include_bytes!`. Guarded on the
# xtask dir because compare-bench.sh checks out arbitrary refs: refs that
# predate the xtask crate build their fixtures during `cargo bench`
# themselves, so the explicit step is skipped for them.
if [ -d xtask ]; then
  echo "building wasm test fixtures via xtask" | tee -a "$log"
  cargo xtask build-fixtures 2>&1 | tee -a "$log"
fi

# gungraun: pin to the isolated CPU. The criterion-style benches are
# multi-threaded (tokio + hyper) and would lose throughput under taskset,
# so they run unpinned across the non-isolated cores.
prefix=()
if [ "$bench" = "gungraun" ]; then
  if [ -x "$(command -v taskset)" ]; then
    prefix=(taskset -c "$isolated_cpu")
    echo "pinning $bench to CPU $isolated_cpu via taskset" | tee -a "$log"
  else
    echo "::warning::taskset not installed; running $bench unpinned" | tee -a "$log"
  fi
fi

"${prefix[@]}" cargo bench -p wash-runtime --features wasip3 --bench "$bench" 2>&1 \
  | tee -a "$log"

echo "WASMCLOUD_BENCH_LOG=${log}" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "WASMCLOUD_BENCH_MARKER=${marker}" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "log: $log"
