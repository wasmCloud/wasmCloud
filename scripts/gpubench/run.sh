#!/usr/bin/env bash
# Run the GPU benchmarks on this box and leave raw results in OUTDIR:
#
#   OUTDIR/metadata.json
#   OUTDIR/gap/<model>/<step>.r<N>.json        llama-bench -o json, one per repetition
#   OUTDIR/gap/<model>/wasm.r<N>.report.json   bench-gpu's report for each wasm repetition
#   OUTDIR/gap/<model>/webgpu-native-mt.r1.json  the headline model on every CPU thread
#   OUTDIR/gap/<model>/webgpu-native-sched.log ggml's scheduler placement, headline model
#   OUTDIR/serverless/<side>.ndjson            bench-gpu turns events, wasmcloud and native
#   OUTDIR/serverless/gpu-<side>.ndjson        GPU memory samples over each side's turns
#
# `bench-tools gpu jsonl OUTDIR` turns it into history rows.
#
#   run.sh [--smoke] [--only gap|serverless] OUTDIR
#
# --smoke runs the 0.5B model with short tests: it proves every step works
# and its numbers are not worth publishing.
#
# Repetitions run one llama-bench run each, rotating the step order every
# repetition, so slow drift (thermals, clocks) spreads across the steps
# instead of landing on whichever runs last. metadata.json records GPU
# clocks and temperature around each part, and how much of each model the
# page cache held. GPUBENCH_LOCK_CLOCKS=<MHz> locks an NVIDIA GPU's clocks
# for the run (needs passwordless sudo).
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
  gap_models=(qwen2.5-0.5b-q4km)
  bench_args=(-p 64 -n 16)
  reps=2
  turn_models=(qwen2.5-0.5b-q4km)
  rounds=1
  idle_ms=3000
else
  gap_model=qwen2.5-7b-q4km
  gap_models=(qwen2.5-7b-q4km qwen2.5-1.5b-q4km qwen2.5-0.5b-q4km)
  bench_args=(-p 512 -n 128)
  reps=5
  turn_models=(qwen2.5-7b-q4km qwen2.5-1.5b-q4km qwen2.5-0.5b-q4km)
  rounds=3
  idle_ms=20000
fi
steps=(vendor webgpu-native wasm)

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

# meta_append KEY JSON: append JSON to metadata.json's KEY array.
meta_append() {
  jq --arg k "$1" --argjson v "$2" '.[$k] += [$v]' "$out/metadata.json" >"$out/metadata.json.part"
  mv "$out/metadata.json.part" "$out/metadata.json"
}

# gpu_state WHEN: record clocks, temperature and power, where the box says.
gpu_state() {
  local q
  case "$kind" in
    cuda)
      q="$(nvidia-smi -i "${GPUBENCH_GPU:-0}" --format=csv,noheader,nounits \
        --query-gpu=clocks.sm,clocks.mem,clocks.max.sm,temperature.gpu,power.draw,pstate,clocks_throttle_reasons.active 2>/dev/null)" || return 0
      meta_append gpu_state "$(jq -n --arg when "$1" --arg q "$q" \
        '($q | split(", ")) as $f | {when: $when, sm_mhz: ($f[0] | tonumber? // null),
          mem_mhz: ($f[1] | tonumber? // null), max_sm_mhz: ($f[2] | tonumber? // null),
          temp_c: ($f[3] | tonumber? // null), power_w: ($f[4] | tonumber? // null),
          pstate: $f[5], throttle: $f[6]}')"
      ;;
    metal)
      # No clocks without root; the thermal pressure level is what macOS gives.
      q="$(pmset -g therm 2>/dev/null | tr '\n' ' ')" || return 0
      meta_append gpu_state "$(jq -n --arg when "$1" --arg q "$q" '{when: $when, therm: $q}')"
      ;;
  esac
}

# page_cache WHEN MODEL_PATH: record how much of the file the page cache held.
page_cache() {
  local cached=null
  [ -f "$2" ] || return 0
  if command -v fincore >/dev/null 2>&1; then
    cached="$(fincore --bytes --noheadings --output RES "$2" 2>/dev/null | tr -d ' ')" || cached=null
    [ -n "$cached" ] || cached=null
  fi
  meta_append page_cache "$(jq -n --arg when "$1" --arg f "$(basename "$2")" \
    --argjson c "$cached" --argjson b "$(wc -c <"$2" | tr -d ' ')" \
    '{when: $when, file: $f, cached_bytes: $c, bytes: $b}')"
}

lock_clocks() {
  local mhz="${GPUBENCH_LOCK_CLOCKS:-}"
  [ -n "$mhz" ] && [ "$kind" = cuda ] || return 0
  if sudo -n nvidia-smi -i "${GPUBENCH_GPU:-0}" -lgc "$mhz,$mhz" >&2; then
    jq --argjson m "$mhz" '.locked_clocks_mhz = $m' "$out/metadata.json" >"$out/metadata.json.part"
    mv "$out/metadata.json.part" "$out/metadata.json"
    trap 'sudo -n nvidia-smi -i "${GPUBENCH_GPU:-0}" -rgc >&2 || true' EXIT
  else
    log "could not lock GPU clocks; running unlocked"
  fi
}

jq -n \
  --arg box "$box" \
  --arg gpu "$(gpu_name)" \
  --arg os "$(uname -sr)" \
  --arg sha "$(git -C "$repo_root" rev-parse HEAD)" \
  --arg llama "$LLAMA_CPP_COMMIT" \
  --arg llama_wit "$LLAMA_WIT_COMMIT" \
  --arg wgpu "$(host_wgpu_versions "$repo_root/Cargo.lock" | tr '\n' ' ')" \
  --arg smoke "$smoke" \
  --arg gap_model "$gap_model" \
  --argjson reps "$reps" \
  --arg started "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{box: $box, gpu: $gpu, os: $os, sha: $sha, llama_cpp: $llama,
    llama_wit: $llama_wit, wgpu: $wgpu,
    smoke: ($smoke == "yes"), gap_model: $gap_model, reps: $reps,
    gpu_state: [], page_cache: [], started: $started}' >"$out/metadata.json"
lock_clocks

# gap_run STEP MODEL DIR NAME ARGS...: one llama-bench run of STEP to DIR/NAME.json.
gap_run() {
  local step="$1" model="$2" dir="$3" name="$4" bin
  shift 4
  bin="$("$gpubench_dir/build/llama.sh" "$step")"
  case "$step" in
    vendor | webgpu-native)
      "$bin/llama-bench" -m "$model" "$@" -ngl 99 -o json \
        >"$dir/$name.json" 2>"$dir/$name.log"
      ;;
    wasm)
      "$bench_gpu" run --dir "$(dirname "$model"):/models" --report "$dir/$name.report.json" \
        "$bin/llama-bench" -- llama-bench -m "/models/$(basename "$model")" \
        "$@" -ngl 99 -o json \
        >"$dir/$name.json" 2>"$dir/$name.log"
      ;;
  esac
}

run_gap() {
  local name model dir rep i step order
  for step in "${steps[@]}"; do
    # Build first, so a build failure isn't charged to a repetition.
    attempt "gap/build/$step" "$gpubench_dir/build/llama.sh" "$step" >/dev/null
  done
  for name in "${gap_models[@]}"; do
    if ! model="$("$gpubench_dir/fetch-model.sh" "$name")"; then
      log "gap: fetching $name failed"
      failed+=("gap/$name")
      continue
    fi
    dir="$out/gap/$name"
    mkdir -p "$dir"
    page_cache "gap/$name" "$model"
    gpu_state "gap/$name/start"
    for rep in $(seq 1 "$reps"); do
      # Rotate the order every repetition.
      order=()
      for i in 0 1 2; do
        order+=("${steps[$(((rep - 1 + i) % 3))]}")
      done
      for step in "${order[@]}"; do
        log "gap: $name $step rep $rep/$reps"
        # One CPU thread for both WebGPU steps, as the wasm guest has: they
        # then differ only in what implements webgpu.h.
        case "$step" in
          vendor) attempt "gap/$name/$step" gap_run "$step" "$model" "$dir" "$step.r$rep" "${bench_args[@]}" -r 1 ;;
          *) attempt "gap/$name/$step" gap_run "$step" "$model" "$dir" "$step.r$rep" "${bench_args[@]}" -r 1 -t 1 ;;
        esac
      done
    done
    gpu_state "gap/$name/end"
    if [ "$name" = "$gap_model" ]; then
      log "gap: $name webgpu-native on $(ncpu) threads"
      attempt "gap/$name/webgpu-native-mt" gap_run webgpu-native "$model" "$dir" webgpu-native-mt.r1 \
        "${bench_args[@]}" -r 2 -t "$(ncpu)"
      # Where ggml places each op. Short, as it logs every graph; the log is
      # debug level, which llama-bench prints only with -v.
      log "gap: $name scheduler placement"
      GGML_SCHED_DEBUG=2 attempt "gap/$name/sched" gap_run webgpu-native "$model" "$dir" webgpu-native-sched \
        -p 64 -n 0 -r 1 -t 1 -v
    fi
  done
}

write_plan() {
  local plan="$1" wasm native name model turns=()
  wasm="$("$gpubench_dir/build/llama.sh" wasm)"
  native="$("$gpubench_dir/build/llama.sh" webgpu-native)"
  for name in "${turn_models[@]}"; do
    model="$("$gpubench_dir/fetch-model.sh" "$name")"
    turns+=("$(jq -n --arg n "$name" --arg w "$wasm/llama-simple" --arg x "$native/llama-simple" \
      --arg m "/models/$(basename "$model")" \
      '{name: $n, wasm: $w, native: $x, args: ["llama-simple", "-m", $m, "-n", "1", "-ngl", "99", "Hello"]}')")
  done
  printf '%s\n' "${turns[@]}" |
    jq -s --argjson r "$rounds" --argjson i "$idle_ms" '{turns: ., rounds: $r, idle_ms: $i}' >"$plan"
}

# serve SIDE: one side's turns, with the GPU sampled from before it starts.
serve() {
  local side="$1" dir="$out/serverless" sampler flag=()
  [ "$side" = native ] && flag=(--native)
  "$gpubench_dir/sample-gpu.sh" "$dir/gpu-$side.ndjson" 200 &
  sampler=$!
  log "serverless: $side baseline"
  sleep 5
  gpu_state "serverless/$side/start"
  log "serverless: $side turns"
  attempt "serverless/$side" "$bench_gpu" turns --dir "$model_dir:/models" --plan "$dir/plan.json" \
    --events "$dir/$side.ndjson" ${flag[@]+"${flag[@]}"} >"$dir/$side.log" 2>&1
  gpu_state "serverless/$side/end"
  sleep 5
  kill "$sampler" 2>/dev/null || true
  wait "$sampler" 2>/dev/null || true
}

run_serverless() {
  local dir="$out/serverless" name
  mkdir -p "$dir"
  rm -f "$dir/plan.json"
  attempt serverless/plan write_plan "$dir/plan.json"
  if [ ! -s "$dir/plan.json" ]; then
    return
  fi
  for name in "${turn_models[@]}"; do
    page_cache serverless "$model_dir/$(jq -r --arg n "$name" '.[$n].file' "$gpubench_dir/models.json")"
  done
  serve wasmcloud
  serve native
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
