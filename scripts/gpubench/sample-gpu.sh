#!/usr/bin/env bash
# Sample GPU memory until killed, one JSON object per line:
#   {"t_ms": <unix ms>, "used": <bytes>, "total": <bytes>}
#
#   sample-gpu.sh OUT.ndjson [INTERVAL_MS]
#
# cuda: nvidia-smi's memory.used for GPU 0 (GPUBENCH_GPU picks another).
# metal: the IOAccelerator's "In use system memory". Apple GPUs share system
# memory, so this counts every process's GPU allocations, the window server's
# included; the report subtracts the idle baseline sampled before a run.

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

out="${1:?usage: sample-gpu.sh OUT.ndjson [INTERVAL_MS]}"
interval_ms="${2:-200}"
: >"$out"

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

case "$(gpubench_kind)" in
  cuda)
    gpu="${GPUBENCH_GPU:-0}"
    # Killing this script must stop nvidia-smi too. A foreground pipeline
    # would hold off the trap until it ended, so run it as a job and wait.
    trap 'pkill -P $$ 2>/dev/null; exit 0' TERM INT
    nvidia-smi -i "$gpu" --query-gpu=memory.used,memory.total --format=csv,noheader,nounits \
      -lms "$interval_ms" | while IFS=', ' read -r used total; do
      printf '{"t_ms":%s,"used":%s,"total":%s}\n' "$(now_ms)" "$((used << 20))" "$((total << 20))"
    done >>"$out" &
    wait
    ;;
  metal)
    total="$(sysctl -n hw.memsize)"
    interval="$(awk -v ms="$interval_ms" 'BEGIN { printf "%.3f", ms / 1000 }')"
    while :; do
      used="$(ioreg -r -d 1 -c IOAccelerator | grep -o '"In use system memory"=[0-9]*' | head -1 | cut -d= -f2)"
      printf '{"t_ms":%s,"used":%s,"total":%s}\n' "$(now_ms)" "${used:-0}" "$total" >>"$out"
      sleep "$interval"
    done
    ;;
  *) die "no GPU memory counter for kind '$(gpubench_kind)'" ;;
esac
