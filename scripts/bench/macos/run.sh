#!/usr/bin/env bash
# Reproduce the macOS scale_density results in README.md.
#
#   scripts/bench/macos/run.sh density   # max warm and scale-to-zero at a memory limit
#   scripts/bench/macos/run.sh cores     # throughput across core counts
#   scripts/bench/macos/run.sh stock     # CI's own settings, for comparison
#
# MEMORY_LIMIT (default 32GiB) sets max-process-memory; keep it well under
# physical RAM. CORES (default 1,2,4,6,12,18) is the sweep for `cores`.
# Results land in $OUT (default target/macos-bench/<preset>.{log,json}).
set -euo pipefail

preset="${1:?usage: run.sh density|cores|stock}"
root="$(git rev-parse --show-toplevel)"
out="${OUT:-$root/target/macos-bench}"
mkdir -p "$out"
cd "$root"

# Fixture: the HTTP handler every run uses.
[ -f crates/wash-runtime/tests/wasm/http_handler_p3.wasm ] || cargo xtask build-fixtures http-handler-p3

limit="${MEMORY_LIMIT:-32GiB}"

# Dense warm layout: a 2 MiB reservation per guest memory, no guard, growth
# mode, and a pool pinned to a count memory can fill. A 2 MiB heap cap and
# bounds checks with no guard behind them: fine for this fixture, not a
# production default (README.md, "Trade-offs").
dense=(WASH_SCALE_GUEST_MEMORY_MODE=growth WASH_SCALE_MEMORY_RESERVATION=2MiB
       WASH_SCALE_MEMORY_GUARD=0 WASH_SCALE_MAX_MEMORIES=300000)

# Reset freed slots in place rather than remapping them. Linear memory only
# takes effect with the patched wasmtime (wasmtime/README.md); harmless
# without it. Set either to 0 to compare against the old path.
keep=(WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT="${WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT:-262144}"
      WASMTIME_POOLING_TABLE_KEEP_RESIDENT="${WASMTIME_POOLING_TABLE_KEEP_RESIDENT:-65536}")

# Steps big enough to load every core: CI's 250 starts and 200 requests per
# step finish in milliseconds.
load=(WASH_SCALE_START_CONCURRENCY=64 WASH_SCALE_STEP=10000 WASH_SCALE_SAMPLE=20000)

bench() { # name flavour VAR=value...
  local name=$1 flavour=$2; shift 2
  echo "=== $name: $* ===" | tee "$out/$name.log"
  env "$@" WASH_SCALE_REPORT_JSON="$out/$name.json" \
    cargo bench -q -p wash-runtime --bench scale_density -- "$flavour" 2>&1 | tee -a "$out/$name.log"
}

case "$preset" in
  density)
    bench density-warm warm WASH_SCALE_MEMORY_LIMIT="$limit" WASH_SCALE_MAX_SECS=1800 \
      "${dense[@]}" "${keep[@]}" "${load[@]}"
    bench density-s2z scale-to-zero WASH_SCALE_MEMORY_LIMIT="$limit" WASH_SCALE_MAX_SECS=1800 \
      "${keep[@]}" "${load[@]}" ;;
  cores)
    cores="${CORES:-1,2,4,6,12,18}"
    bench cores-warm warm WASH_SCALE_CPUS="$cores" WASH_SCALE_MEMORY_LIMIT=8GiB \
      WASH_SCALE_MAX_COMPONENTS=50000 "${dense[@]}" "${keep[@]}" "${load[@]}"
    bench cores-s2z scale-to-zero WASH_SCALE_CPUS="$cores" WASH_SCALE_MEMORY_LIMIT=8GiB \
      WASH_SCALE_MAX_COMPONENTS=100000 "${keep[@]}" "${load[@]}" ;;
  stock)
    bench stock "" ;;
  *) echo "unknown preset: $preset" >&2; exit 2 ;;
esac
