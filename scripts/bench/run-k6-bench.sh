#!/usr/bin/env bash
# Run a single k6 bench against runtime-operator deployed in the host's
# k3s cluster. Mirrors run-bench.sh's contract:
#
#   - takes a bench name (e.g. `k6_http_invoke`) as the only positional arg
#   - emits $CARGO_TARGET_DIR/k6/summary.json (consumed by `bench-tools`)
#   - writes a run log under $CARGO_TARGET_DIR/run-${bench}-${run_id}.log
#   - writes a marker file at $CARGO_TARGET_DIR/.bench-start-${bench} so
#     parity with criterion's stale-data filter is preserved (the k6
#     parser ignores the marker, but bench-push-results.mjs and
#     compare-bench.sh rely on the file existing)
#
# Cluster lifecycle:
#   - k3s.service is assumed up (provisioned by ansible). We do NOT
#     start/stop k3s itself; that's a multi-second operation we can't
#     amortize across runs and `k3s-killall.sh && systemctl restart k3s`
#     is what the operator runs by hand if a bench corrupts state.
#   - WASMCLOUD_BENCH_K6_RESET_CLUSTER=1 force-restarts k3s before the
#     run for those rare cases.
#   - The runtime-operator chart is reinstalled fresh each run (so e.g.
#     a chart-side regression doesn't sit cached in the cluster).
#   - The bench namespace + workload are deleted at the end (success or
#     fail) so the next run starts from a known empty state.
#
# Reads (required by k6, set here from the bench script's name):
#   K6_TARGET_URL    - URL the k6 script hits
#
# Reads (optional):
#   CARGO_TARGET_DIR        default /var/lib/bench/target
#   GITHUB_RUN_ID           default "local"
#   WASMCLOUD_K6_TIMEOUT    default 5m (overall k6 invocation timeout)
#   K6_RPS / K6_DURATION    forwarded to the k6 script (see k6/http_invoke.js)

set -euo pipefail

bench="${1:?bench name required (e.g. k6_http_invoke)}"

# Strip the `k6_` prefix to get the script basename. We dispatch on the
# prefix in run-bench.sh + bench-tools so adding a new k6 bench is just
# `scripts/bench/k6/<scenario>.js` + a new option in bench.yml.
case "$bench" in
  k6_*) script_name="${bench#k6_}" ;;
  *)
    echo "::error::run-k6-bench.sh called with non-k6 bench name: $bench" >&2
    exit 1
    ;;
esac

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
script_path="${repo_root}/scripts/bench/k6/${script_name}.js"
if [ ! -f "$script_path" ]; then
  echo "::error::no k6 script at ${script_path}" >&2
  exit 1
fi

: "${CARGO_TARGET_DIR:=/var/lib/bench/target}"
mkdir -p "$CARGO_TARGET_DIR"

k6_dir="${CARGO_TARGET_DIR}/k6"
marker="${CARGO_TARGET_DIR}/.bench-start-${bench}"
log="${CARGO_TARGET_DIR}/run-${bench}-${GITHUB_RUN_ID:-local}.log"

# Wipe the previous run's k6 output before the new one — the parser
# reads `summary.json` from this dir unconditionally, so a stale file
# from a previous run would otherwise be reported as the current
# bench's result if k6 failed before writing its own.
rm -rf "$k6_dir"
mkdir -p "$k6_dir"

# Marker for parity with criterion benches (compare-bench.sh + GHA
# step-summary check both reference these). For k6 the parser ignores
# the marker because we already wipe `k6_dir` above.
touch "$marker"

bench_ns="wasmcloud-bench"
release_name="operator-bench"
bench_chart="${repo_root}/charts/runtime-operator"
bench_values="${repo_root}/scripts/bench/k6/values.bench.yaml"
bench_workload="${repo_root}/scripts/bench/k6/workloads/hello.yaml"

{
  echo "=== run-k6-bench: ${bench} ==="
  echo "host:    $(hostname)"
  echo "kernel:  $(uname -srm)"
  echo "cpu:     $(awk -F: '/^model name/{print $2; exit}' /proc/cpuinfo | sed 's/^ //')"
  echo "online:  $(nproc) cpu(s)"
  echo "git:     $(git -C "$repo_root" rev-parse HEAD)"
  echo "ref:     ${GITHUB_REF_NAME:-?}"
  echo "ts:      $(date -u +%FT%TZ)"
  echo "k6:      $(k6 version | head -1)"
  echo "kubectl: $(kubectl version --client=true 2>/dev/null | head -1)"
  echo "helm:    $(helm version --short 2>/dev/null)"
  echo "k3s:     $(k3s --version 2>/dev/null | head -1)"
  echo
} | tee "$log"

# k3s writes its kubeconfig with mode 600 owned by root. The runner
# user (`bench`) reads via KUBECONFIG pointing at the world-readable
# copy installed by ansible at /etc/rancher/k3s/k3s.yaml.user.
export KUBECONFIG="${KUBECONFIG:-/etc/rancher/k3s/k3s.yaml.user}"
if [ ! -r "$KUBECONFIG" ]; then
  echo "::error::KUBECONFIG=$KUBECONFIG not readable; was ansible/provision.yml --tags k8s run?" >&2
  exit 1
fi

if [ "${WASMCLOUD_BENCH_K6_RESET_CLUSTER:-0}" = "1" ]; then
  echo "WASMCLOUD_BENCH_K6_RESET_CLUSTER=1 — resetting k3s" | tee -a "$log"
  sudo /usr/local/bin/k3s-killall.sh 2>&1 | tee -a "$log" || true
  sudo systemctl restart k3s 2>&1 | tee -a "$log"
  # k3s comes back fast; poll the API until it answers.
  for _ in $(seq 1 30); do
    if kubectl --request-timeout=3s get --raw=/readyz >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
fi

# Always tear the bench namespace down, even on failure — leaves the
# cluster in the same state run-to-run regardless of how the previous
# run exited.
cleanup() {
  echo "--- cleanup ---" | tee -a "$log"
  kubectl delete -n "$bench_ns" -f "$bench_workload" --ignore-not-found --wait=false 2>&1 | tee -a "$log" || true
  helm uninstall -n "$bench_ns" "$release_name" --wait --timeout 60s 2>&1 | tee -a "$log" || true
  kubectl delete namespace "$bench_ns" --ignore-not-found --wait=false 2>&1 | tee -a "$log" || true
}
trap cleanup EXIT

echo "--- helm upgrade --install ${release_name} ---" | tee -a "$log"
kubectl create namespace "$bench_ns" --dry-run=client -o yaml | kubectl apply -f - 2>&1 | tee -a "$log"
helm upgrade --install \
  --namespace "$bench_ns" \
  --create-namespace \
  --values "$bench_values" \
  --wait --timeout 5m \
  "$release_name" "$bench_chart" \
  2>&1 | tee -a "$log"

echo "--- apply workload ---" | tee -a "$log"
# The workload manifest is namespace-less; -n routes both the Service
# and the WorkloadDeployment into the chart's release namespace where
# the operator + host group live.
kubectl apply -n "$bench_ns" -f "$bench_workload" 2>&1 | tee -a "$log"

# Wait for the WorkloadDeployment to become Ready. The CRD reports a
# `READY` column matching `status.conditions[type=Ready]`. Poll instead
# of `wait --for=condition=Ready` because older operator versions don't
# expose that condition path in a wait-compatible way.
echo "--- wait for WorkloadDeployment ready ---" | tee -a "$log"
deadline=$((SECONDS + 120))
while :; do
  ready="$(kubectl -n "$bench_ns" get workloaddeployment hello-bench \
    -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
  if [ "$ready" = "True" ]; then
    echo "workload ready" | tee -a "$log"
    break
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "::error::workload not Ready after 120s" | tee -a "$log"
    kubectl -n "$bench_ns" get all,workloaddeployment,host 2>&1 | tee -a "$log" || true
    kubectl -n "$bench_ns" describe workloaddeployment hello-bench 2>&1 | tee -a "$log" || true
    exit 1
  fi
  sleep 2
done

# k3s gives every Service a ClusterIP reachable from the node itself.
# We're running k6 on the same node, so cluster-ip:port is the
# straightforward target — no NodePort, no port-forward.
service_ip="$(kubectl -n "$bench_ns" get svc hello-bench -o jsonpath='{.spec.clusterIP}')"
service_port="$(kubectl -n "$bench_ns" get svc hello-bench -o jsonpath='{.spec.ports[0].port}')"
target_url="http://${service_ip}:${service_port}/"
echo "k6 target: ${target_url}" | tee -a "$log"

# Warmup: a small burst of requests to JIT the cargo-loaded host pod
# and prime any one-time TLS / wasm component instantiation cost. We
# discard k6's output here — only the steady-state run produces data.
echo "--- warmup (10 req) ---" | tee -a "$log"
for _ in $(seq 1 10); do
  curl --silent --output /dev/null --max-time 5 "$target_url" || true
done

echo "--- k6 run ---" | tee -a "$log"
# k6 writes summary.json (via handleSummary) into its CWD, so cd into
# the k6 output dir first. That dir is also where bench-tools k6::walk
# reads from — symmetric.
(
  cd "$k6_dir"
  K6_TARGET_URL="$target_url" \
    timeout "${WASMCLOUD_K6_TIMEOUT:-5m}" \
    k6 run "$script_path"
) 2>&1 | tee -a "$log"

if [ ! -s "${k6_dir}/summary.json" ]; then
  echo "::error::k6 produced no summary.json at ${k6_dir}/summary.json" | tee -a "$log"
  exit 1
fi

echo "WASMCLOUD_BENCH_LOG=${log}" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "WASMCLOUD_BENCH_MARKER=${marker}" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "log:     $log"
echo "summary: ${k6_dir}/summary.json"
