#!/usr/bin/env bash
# Run the GPU benchmarks on this box and leave raw results in OUTDIR:
#
#   OUTDIR/metadata.json
#   OUTDIR/gap/llama/<step>.json      llama-bench -o json, one per step
#   OUTDIR/gap/llama/wasm.report.json bench-gpu's report for step 3
#   OUTDIR/serverless/wasmcloud.ndjson  bench-gpu turns events
#   OUTDIR/serverless/gpu.ndjson        GPU memory samples over the turns
#
# `bench-tools gpu jsonl OUTDIR` turns it into history rows.
#
#   run.sh [--smoke] [--only gap|serverless] OUTDIR
#
# --smoke runs the 0.5B model with short tests: it proves every step works
# and its numbers are not worth publishing.
#
# A step that fails doesn't stop the others. Failed steps are listed in
# metadata.json's `failed`, and run.sh exits 1 after the last step.

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

smoke=no
only=all
while [ $# -gt 1 ]; do
  case "$1" in
    --smoke) smoke=yes ;;
    --only) only="$2"; shift ;;
    *) die "unknown flag $1" ;;
  esac
  shift
done
out="${1:?usage: run.sh [--smoke] [--only gap|serverless] OUTDIR}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
box="$(gpubench_box)"
kind="$(gpubench_kind)"

if [ "$smoke" = yes ]; then
  gap_model=qwen2.5-0.5b-q4km
  bench_args=(-p 64 -n 16 -r 2)
  turn_models=(qwen2.5-0.5b-q4km)
  rounds=1
  idle_ms=3000
else
  gap_model=qwen2.5-7b-q4km
  bench_args=(-p 512 -n 128 -r 5)
  turn_models=(qwen2.5-7b-q4km qwen2.5-1.5b-q4km qwen2.5-0.5b-q4km)
  rounds=3
  idle_ms=20000
fi

log "building bench-gpu"
(cd "$repo_root" && cargo build -q --release -p bench-gpu)
bench_gpu="$repo_root/target/release/bench-gpu"

failed=()
# attempt NAME CMD...: run CMD with errexit in force, recording NAME if it
# fails. Running it under `if` or `||` would switch errexit off inside it.
attempt() {
  local name="$1" rc
  shift
  set +e
  (set -e; "$@")
  rc=$?
  set -e
  if [ "$rc" -ne 0 ]; then
    log "$name failed (exit $rc)"
    failed+=("$name")
  fi
}

gpu_name() {
  case "$kind" in
    cuda) nvidia-smi --query-gpu=name,driver_version --format=csv,noheader -i "${GPUBENCH_GPU:-0}" ;;
    metal) system_profiler SPDisplaysDataType 2>/dev/null | awk -F': ' '/Chipset Model/ { print $2; exit }' ;;
    *) echo "software ($box)" ;;
  esac
}

jq -n \
  --arg box "$box" \
  --arg gpu "$(gpu_name)" \
  --arg os "$(uname -sr)" \
  --arg sha "$(git -C "$repo_root" rev-parse HEAD)" \
  --arg llama "$LLAMA_CPP_COMMIT" \
  --arg wgpu "$(host_wgpu_versions "$repo_root/Cargo.lock" | tr '\n' ' ')" \
  --arg smoke "$smoke" \
  --arg started "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{box: $box, gpu: $gpu, os: $os, sha: $sha, llama_cpp: $llama, wgpu: $wgpu,
    smoke: ($smoke == "yes"), started: $started}' >"$out/metadata.json"

gap_step() {
  local step="$1" model="$2" dir="$3" bin
  bin="$("$gpubench_dir/build/llama.sh" "$step")"
  log "gap: $step"
  case "$step" in
    vendor)
      "$bin/llama-bench" -m "$model" "${bench_args[@]}" -ngl 99 -o json \
        >"$dir/$step.json" 2>"$dir/$step.log"
      ;;
    webgpu-native)
      # One CPU thread, as the wasm guest has: the two WebGPU steps then
      # differ only in what implements webgpu.h.
      "$bin/llama-bench" -m "$model" "${bench_args[@]}" -ngl 99 -t 1 -o json \
        >"$dir/$step.json" 2>"$dir/$step.log"
      ;;
    wasm)
      "$bench_gpu" run --dir "$(dirname "$model"):/models" --report "$dir/wasm.report.json" \
        "$bin/llama-bench" -- llama-bench -m "/models/$(basename "$model")" \
        "${bench_args[@]}" -ngl 99 -t 1 -o json \
        >"$dir/$step.json" 2>"$dir/$step.log"
      ;;
  esac
}

run_gap() {
  local model dir step
  if ! model="$("$gpubench_dir/fetch-model.sh" "$gap_model")"; then
    log "gap: fetching $gap_model failed"
    failed+=(gap)
    return
  fi
  dir="$out/gap/llama"
  mkdir -p "$dir"
  jq --arg m "$gap_model" '.gap_model = $m' "$out/metadata.json" >"$out/metadata.json.part"
  mv "$out/metadata.json.part" "$out/metadata.json"

  for step in vendor webgpu-native wasm; do
    attempt "gap/$step" gap_step "$step" "$model" "$dir"
  done
}

write_plan() {
  local plan="$1" bin name model turns=()
  bin="$("$gpubench_dir/build/llama.sh" wasm)"
  for name in "${turn_models[@]}"; do
    model="$("$gpubench_dir/fetch-model.sh" "$name")"
    turns+=("$(jq -n --arg n "$name" --arg w "$bin/llama-simple" --arg m "/models/$(basename "$model")" \
      '{name: $n, wasm: $w, args: ["llama-simple", "-m", $m, "-n", "1", "-ngl", "99", "Hello"]}')")
  done
  printf '%s\n' "${turns[@]}" |
    jq -s --argjson r "$rounds" --argjson i "$idle_ms" '{turns: ., rounds: $r, idle_ms: $i}' >"$plan"
}

run_turns() {
  "$bench_gpu" turns --dir "$model_dir:/models" --plan "$2" --events "$1/wasmcloud.ndjson" \
    >"$1/wasmcloud.log" 2>&1
}

run_serverless() {
  local dir plan sampler
  dir="$out/serverless"
  mkdir -p "$dir"
  plan="$dir/plan.json"
  rm -f "$plan"
  attempt serverless/plan write_plan "$plan"
  if [ ! -s "$plan" ]; then
    return
  fi

  "$gpubench_dir/sample-gpu.sh" "$dir/gpu.ndjson" 200 &
  sampler=$!
  trap 'kill "$sampler" 2>/dev/null || true' EXIT
  log "serverless: idle baseline"
  sleep 5
  log "serverless: wasmCloud turns"
  attempt serverless/wasmcloud run_turns "$dir" "$plan"
  sleep 5
  kill "$sampler" 2>/dev/null || true
  wait "$sampler" 2>/dev/null || true
  trap - EXIT
}

case "$only" in
  all) run_gap; run_serverless ;;
  gap) run_gap ;;
  serverless) run_serverless ;;
  *) die "unknown --only '$only'" ;;
esac
jq --arg f "$(date -u +%Y-%m-%dT%H:%M:%SZ)" '.finished = $f | .failed = $ARGS.positional' \
  "$out/metadata.json" --args ${failed[@]+"${failed[@]}"} >"$out/metadata.json.part"
mv "$out/metadata.json.part" "$out/metadata.json"
log "results in $out"
if [ "${#failed[@]}" -gt 0 ]; then
  die "failed: ${failed[*]}"
fi
