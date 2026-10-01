#!/usr/bin/env bash
# Run a k6 load scenario against wasmCloud. The same script runs on a laptop,
# in the k6bench workflow on the Hetzner bench host, and against a customer's
# own cluster. See README.md for the full walkthrough.
#
#   run.sh [run] [options]   bring up (or reuse) the stack, run one scenario
#   run.sh up    [options]   bring up the kind cluster + chart + registry only
#   run.sh down              delete the kind cluster
#
# Results land in bench-results/<utc>_<scenario>_<profile>/ (or --out-dir):
# summary.json, cluster.ndjson, metadata.json, run.log, and raw.ndjson.gz
# with --raw. `bench-tools k6 report <dir>` renders them.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"

usage() {
  cat <<'EOF'
usage: run.sh [run|up|down] [options]

scenario
  --scenario NAME        http-hello | fan-out-10 | chain-5 | many-workloads  [http-hello]
  --profile NAME         constant | stress | spike                            [constant]
  --rate N               requests/s for constant and spike (spike base)       [1000]
  --duration D           measured window, e.g. 90s, 3m                        [2m]
  --warmup D             unreported warm-up before it                         [30s]
  --workloads N          many-workloads only: number of workloads             [100]
  --stress-rates LIST    stress only: comma-separated step rates
  --slo-p99-ms N         p99 ceiling for thresholds and stress                [250]
  --routing MODE         fan-out-10 and chain-5 only: how workloads call each
                         other. local: same-host local routing, by localRoute
                         name; network: through Service DNS, with the host's
                         local routing off                                    [local]

target
  --target kind|kube     kind: this script's own cluster; kube: the current
                         kubectl context (a cluster wasmCloud already runs on) [kind]
  --target-url URL       where k6 sends requests (required for --target kube)
  --namespace NS         namespace for the bench workloads                    [k6bench]
  --registry REF         registry the hosts pull bench components from; with
                         --target kube it must be reachable from the cluster

images
  --wasmcloud-version V  wash + operator image tag from ghcr.io/wasmcloud     [chart appVersion]
  --build-local          build wash + operator images from this tree, kind-load them
  --reuse-stack          keep the installed chart and images as they are (fast
                         iteration on scenarios; needs an existing cluster)

run
  --pin                  pin kind nodes and k6 to CPUs (bench host layout)
  --raw                  also keep k6's per-request JSON stream (large)
  --out-dir DIR          result directory
  --keep                 leave the scenario's workloads deployed afterwards
  --down                 delete the kind cluster afterwards
  --ci                   CI mode: implies --pin --down, no colors
EOF
}

cmd=run
case "${1:-}" in run | up | down) cmd="$1"; shift ;; -h | --help) usage; exit 0 ;; esac

scenario=http-hello
profile=constant
rate=1000
duration=2m
warmup=30s
workloads=100
stress_rates=""
slo_p99_ms=250
routing=local
target=kind
target_url=""
namespace=k6bench
registry=""
wasmcloud_version=""
build_local=0
reuse_stack=0
pin=0
raw=0
out_dir=""
keep=0
down=0
ci=0

while [ $# -gt 0 ]; do
  case "$1" in
    --scenario) scenario="$2"; shift ;;
    --profile) profile="$2"; shift ;;
    --rate) rate="$2"; shift ;;
    --duration) duration="$2"; shift ;;
    --warmup) warmup="$2"; shift ;;
    --workloads) workloads="$2"; shift ;;
    --stress-rates) stress_rates="$2"; shift ;;
    --slo-p99-ms) slo_p99_ms="$2"; shift ;;
    --routing) routing="$2"; shift ;;
    --target) target="$2"; shift ;;
    --target-url) target_url="$2"; shift ;;
    --namespace) namespace="$2"; shift ;;
    --registry) registry="$2"; shift ;;
    --wasmcloud-version) wasmcloud_version="$2"; shift ;;
    --build-local) build_local=1 ;;
    --reuse-stack) reuse_stack=1 ;;
    --pin) pin=1 ;;
    --raw) raw=1 ;;
    --out-dir) out_dir="$2"; shift ;;
    --keep) keep=1 ;;
    --down) down=1 ;;
    --ci) ci=1; pin=1; down=1 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

cluster="${K6BENCH_CLUSTER:-k6bench}"
release=wasmcloud
k6_image="${K6BENCH_K6_IMAGE:-grafana/k6:2.3.0}"
registry_name="${K6BENCH_REGISTRY_NAME:-kind-registry}"
registry_port="${K6BENCH_REGISTRY_PORT:-5001}"
# Bench-host CPU layout (--pin): see README.md. isolcpus= on the bench host
# reserves the k6 CPU; the rest are split between the two kind nodes.
cpus_control_plane="${K6BENCH_CPUS_CONTROL_PLANE:-0}"
cpus_hosts="${K6BENCH_CPUS_HOSTS:-1-4}"
cpus_k6="${K6BENCH_CPUS_K6:-${WASMCLOUD_BENCH_ISOLATED_CPU:-5}}"

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required (see README.md prerequisites)"; }

kctl() { kubectl --context "kind-$cluster" "$@"; }
[ "$target" = kube ] && kctl() { kubectl "$@"; }

cluster_exists() { kind get clusters 2>/dev/null | grep -qx "$cluster"; }

local_routing() { [ "$routing" = local ] && echo true || echo false; }

chart_version() { sed -n 's/^appVersion: *"\{0,1\}\([^"]*\)"\{0,1\}$/\1/p' "$repo/charts/runtime-operator/Chart.yaml"; }

# --- kind stack -------------------------------------------------------------

ensure_registry() {
  if [ -z "$(docker ps -q -f "name=^${registry_name}$")" ]; then
    if [ -n "$(docker ps -aq -f "name=^${registry_name}$")" ]; then
      docker start "$registry_name" >/dev/null
    else
      log "starting local registry $registry_name on 127.0.0.1:$registry_port"
      docker run -d --restart=always -p "127.0.0.1:${registry_port}:5000" \
        --name "$registry_name" registry:2 >/dev/null
    fi
  fi
  if ! docker inspect "$registry_name" -f '{{json .NetworkSettings.Networks}}' | grep -q '"kind"'; then
    docker network connect kind "$registry_name"
  fi
}

# Pods can't resolve a Docker container name, so hosts pull by kind-network IP.
registry_in_cluster() {
  echo "$(docker inspect "$registry_name" -f '{{(index .NetworkSettings.Networks "kind").IPAddress}}'):5000"
}

build_images() {
  local tag ctx
  ctx="$(mktemp -d)"
  # git ls-files, not the directory: .dockerignore skips only target/, and
  # examples/*/target and fixture targets are several GB. COPYFILE_DISABLE
  # keeps macOS tar from adding ._* files, which break WIT bindgen.
  (cd "$repo" && git ls-files -z --cached --others --exclude-standard |
    COPYFILE_DISABLE=1 tar -cf "$ctx/src.tar" --null -T -)
  # Tagged by the context, not HEAD: uncommitted changes must get a new tag
  # or the chart upgrade leaves the host pods on the previous image.
  tag="k6bench-$(sha256 "$ctx/src.tar")"
  log "building wash and runtime-operator images ($tag) from the working tree"
  mkdir "$ctx/src" && tar -xf "$ctx/src.tar" -C "$ctx/src"
  docker build -t "wash:$tag" "$ctx/src" >&2
  docker build -t "runtime-operator:$tag" "$ctx/src/runtime-operator" >&2
  rm -rf "$ctx"
  kind load docker-image --name "$cluster" "wash:$tag" "runtime-operator:$tag" >&2
  image_args="--set runtime.image.registry= --set runtime.image.repository=wash
    --set runtime.image.tag=$tag --set runtime.image.pull_policy=Never
    --set operator.image.registry= --set operator.image.repository=runtime-operator
    --set operator.image.tag=$tag --set operator.image.pull_policy=Never"
  wash_image="wash:$tag"
  operator_image="runtime-operator:$tag"
}

pin_nodes() {
  log "pinning $cluster-control-plane to CPU $cpus_control_plane, $cluster-worker to CPUs $cpus_hosts"
  docker update --cpuset-cpus "$cpus_control_plane" "$cluster-control-plane" >/dev/null
  docker update --cpuset-cpus "$cpus_hosts" "$cluster-worker" >/dev/null
}

stack_up() {
  need docker; need kind; need kubectl; need helm
  if cluster_exists; then
    log "reusing kind cluster $cluster"
    if [ "$reuse_stack" = 1 ]; then
      ensure_registry
      wash_image="$(kctl -n "$namespace" get deploy hostgroup-default -o jsonpath='{.spec.template.spec.containers[0].image}')"
      operator_image="$(kctl -n "$namespace" get deploy runtime-operator -o jsonpath='{.spec.template.spec.containers[0].image}')"
      local installed=false
      kctl -n "$namespace" get deploy hostgroup-default -o jsonpath='{.spec.template.spec.containers[0].args}' |
        grep -q -- --http-local-routing && installed=true
      [ "$installed" = "$(local_routing)" ] ||
        die "installed hosts have local routing $installed; rerun without --reuse-stack for --routing $routing"
      log "reusing installed chart ($wash_image)"
      return
    fi
  else
    log "creating kind cluster $cluster"
    kind create cluster --name "$cluster" --config "$here/kind-config.yaml" --wait 120s >&2
  fi
  ensure_registry
  [ "$pin" = 1 ] && pin_nodes

  local version="${wasmcloud_version:-$(chart_version)}"
  image_args="--set runtime.image.tag=$version --set operator.image.tag=$version"
  host_args="--set runtime.hostGroups[0].http.localBypassRouting=$(local_routing)"
  wash_image="ghcr.io/wasmcloud/wash:$version"
  operator_image="ghcr.io/wasmcloud/runtime-operator:$version"
  [ "$build_local" = 1 ] && build_images

  log "installing runtime-operator chart ($wash_image, local routing $(local_routing))"
  # shellcheck disable=SC2086 # image_args and host_args are lists of flags
  helm upgrade --install "$release" "$repo/charts/runtime-operator" \
    --kube-context "kind-$cluster" -n "$namespace" --create-namespace \
    -f "$here/values.yaml" $image_args $host_args --wait --timeout 5m >&2
}

stack_down() {
  if cluster_exists; then
    log "deleting kind cluster $cluster"
    kind delete cluster --name "$cluster" >&2
  fi
}

# --- components + manifests -------------------------------------------------

sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -c1-12; }

# `wash oci push`, from PATH (or $WASH) when there is one, else from the wash
# image. The container pushes over the kind network, so it needs no port on
# the host and works the same on macOS and Linux.
oci_push() {
  local ref="$1" wasm="$2" wash="${WASH:-}"
  [ -z "$wash" ] && command -v wash >/dev/null 2>&1 && wash=wash
  if [ -n "$wash" ]; then
    "$wash" oci push --insecure "$ref" "$wasm" >/dev/null
    return
  fi
  local net_arg=""
  [ "$target" = kind ] && net_arg="--network kind"
  # shellcheck disable=SC2086
  docker run --rm $net_arg -v "$(dirname "$wasm"):/w:ro" \
    "ghcr.io/wasmcloud/wash:$(chart_version)" \
    oci push --insecure "$ref" "/w/$(basename "$wasm")" >/dev/null
}

# Build and push the bench components. The tag is the wasm's content hash: a
# host caches an OCI artifact by tag, so re-pushing a changed component under
# the same tag would not take effect.
push_components() {
  local push_to="$1" pull_from="$2" name wasm tag
  log "building bench components"
  (cd "$here/components" && cargo build --release --target wasm32-wasip2 --quiet >&2)
  for name in hello relay; do
    wasm="$here/components/target/wasm32-wasip2/release/k6bench_${name}.wasm"
    tag="$(sha256 "$wasm")"
    oci_push "$push_to/k6bench/$name:$tag" "$wasm"
    eval "image_${name}=\"$pull_from/k6bench/$name:$tag\""
  done
}

# __CALL_DOMAIN__ is what a relay calls its targets by: their `.internal`
# localRoute names, or `<service>.<namespace>`, which cluster DNS resolves to
# the Service and the host serves as a Host alias.
render() {
  local file="$1" domain=internal
  [ "$routing" = network ] && domain="$namespace"
  # shellcheck disable=SC2154 # image_hello/image_relay are set by push_components
  sed -e "s|__HELLO_IMAGE__|$image_hello|g" -e "s|__RELAY_IMAGE__|$image_relay|g" \
    -e "s|__CALL_DOMAIN__|$domain|g" "$file"
}

manifests() {
  local base="$here/manifests/$scenario" count=0 i
  case "$scenario" in
    fan-out-10) count=10 ;;
    many-workloads) count="$workloads" ;;
  esac
  [ -f "$base.yaml" ] && render "$base.yaml"
  # many-workloads' first Service is the NodePort all its traffic enters by.
  local svc_type nodeport
  i=0
  while [ "$i" -lt "$count" ]; do
    svc_type=ClusterIP nodeport=""
    [ "$i" = 0 ] && svc_type=NodePort nodeport="      nodePort: 30950"
    echo "---"
    render "$base.each.yaml" | sed -e "s|__I__|$i|g" -e "s|__SVC_TYPE__|$svc_type|" \
      -e "s|^__NODEPORT__\$|$nodeport|" | sed '/^$/d'
    i=$((i + 1))
  done
}

default_host() {
  case "$scenario" in
    http-hello) echo hello.k6bench ;;
    fan-out-10) echo fanout.k6bench ;;
    chain-5) echo chain.k6bench ;;
    many-workloads) echo hello-0.k6bench ;;
  esac
}

# --- k6 ---------------------------------------------------------------------

# Returns how k6 should run and where it should send traffic, as
# "<native|docker> <url>". On Linux the worker's IP is reachable from the host
# and skips Docker's userland port proxy; elsewhere a dockerized k6 on the kind
# network gets the same direct path.
k6_mode() {
  if [ "$target" = kube ]; then
    if command -v k6 >/dev/null 2>&1; then echo "native $target_url"; else echo "docker $target_url"; fi
    return
  fi
  local ip i
  ip="$(docker inspect "$cluster-worker" -f '{{(index .NetworkSettings.Networks "kind").IPAddress}}')"
  # The NodePort can lag the workloads' Ready condition; retry so an early
  # probe doesn't swap in a dockerized k6.
  if command -v k6 >/dev/null 2>&1; then
    for i in 1 2 3 4 5 6 7 8 9 10; do
      if curl -s -o /dev/null -m 2 "http://$ip:30950/"; then
        echo "native http://$ip:30950"
        return
      fi
      [ "$i" = 10 ] || sleep 2
    done
  fi
  echo "docker http://$cluster-worker:30950"
}

# Five 200s in a row, not one: a host pod rolled by a chart upgrade can
# answer once more while its replacement is still picking up the workloads.
wait_routable() {
  local url="$1" host="$2" mode="$3" i=0 ok=0 code
  while [ "$i" -lt 90 ]; do
    if [ "$mode" = native ]; then
      code="$(curl -s -o /dev/null -w '%{http_code}' -m 2 -H "Host: $host" "$url/" || true)"
    else
      code="$(docker run --rm --network kind curlimages/curl:8.11.1 \
        -s -o /dev/null -w '%{http_code}' -m 2 -H "Host: $host" "$url/" 2>/dev/null || true)"
    fi
    if [ "$code" = 200 ]; then
      ok=$((ok + 1))
      [ "$ok" -ge 5 ] && return 0
    else
      ok=0
    fi
    sleep 1
    i=$((i + 1))
  done
  die "$host never answered 200 at $url (last: $code)"
}

# About one sample a second of every kind node (and a dockerized k6) until
# killed. One `docker stats` per container, so a container that isn't up yet
# (k6, before it starts) doesn't cost the others their sample.
sample_cluster() {
  local out="$1" name
  shift
  while :; do
    for name in "$@"; do
      docker stats --no-stream --format \
        "{\"ts\":$(date +%s),\"name\":\"{{.Name}}\",\"cpu\":\"{{.CPUPerc}}\",\"mem\":\"{{.MemUsage}}\"}" \
        "$name" >>"$out" 2>/dev/null &
    done
    wait
    sleep 1
  done
}

# A native k6's CPU and RSS each second from /proc, in docker stats' format
# under the name a dockerized k6 has, so the saturation check sees the
# busiest window, not the run's average. Linux only; a no-op elsewhere.
sample_native_k6() {
  local out="$1" name="$2" hz pid="" prev_ticks="" prev_up="" ticks up rss
  [ -r /proc/uptime ] || return 0
  hz="$(getconf CLK_TCK)"
  while :; do
    if [ -z "$pid" ] || [ ! -r "/proc/$pid/stat" ]; then
      pid="$(pgrep -xn k6 || true)"
      prev_ticks=""
      [ -n "$pid" ] || { sleep 1; continue; }
    fi
    # utime + stime are fields 14 and 15; comm ("k6") has no spaces.
    ticks="$(awk '{ print $14 + $15 }' "/proc/$pid/stat" 2>/dev/null)" || { pid=""; continue; }
    rss="$(awk '/^VmRSS:/ { print $2 }' "/proc/$pid/status" 2>/dev/null)"
    up="$(cut -d' ' -f1 /proc/uptime)"
    if [ -n "$prev_ticks" ] && [ -n "$ticks" ] && [ -n "$rss" ]; then
      awk -v ts="$(date +%s)" -v name="$name" -v hz="$hz" -v rss="$rss" \
        -v dt="$ticks" -v pt="$prev_ticks" -v up="$up" -v pu="$prev_up" \
        'BEGIN { if (up > pu) printf "{\"ts\":%d,\"name\":\"%s\",\"cpu\":\"%.2f%%\",\"mem\":\"%.1fMiB / 0B\"}\n", ts, name, (dt - pt) / hz / (up - pu) * 100, rss / 1024 }' \
        >>"$out"
    fi
    prev_ticks="$ticks"
    prev_up="$up"
    sleep 1
  done
}

run_k6() {
  local mode="$1" url="$2" script="scenarios/$scenario.js" k6_env status
  k6_env="-e TARGET_URL=$url -e PROFILE=$profile -e RATE=$rate -e DURATION=$duration
    -e WARMUP=$warmup -e WORKLOADS=$workloads -e SLO_P99_MS=$slo_p99_ms"
  [ -n "$stress_rates" ] && k6_env="$k6_env -e STRESS_RATES=$stress_rates"
  # The progress bar is one line per second in a log; the summary is enough.
  [ "${K6BENCH_PROGRESS:-0}" = 1 ] || k6_env="$k6_env --quiet"
  [ "$ci" = 1 ] && k6_env="$k6_env --no-color"

  set +e
  if [ "$mode" = native ]; then
    local raw_arg="" pin_cmd=""
    [ "$raw" = 1 ] && raw_arg="--out json=$out_dir/raw.ndjson.gz"
    [ "$pin" = 1 ] && command -v taskset >/dev/null 2>&1 && pin_cmd="taskset -c $cpus_k6"
    if [ "$pin" = 1 ]; then
      pin_cmd="env GOMAXPROCS=1"
      command -v taskset >/dev/null 2>&1 && pin_cmd="$pin_cmd taskset -c $cpus_k6"
    fi
    # `times` reports the subshell's children, which is k6 alone: the CPU it
    # used, for the generator-saturation check. A dockerized k6 is sampled.
    # shellcheck disable=SC2086 # word-split flag lists
    (
      cd "$here" && $pin_cmd k6 run $k6_env -e "OUT_DIR=$out_dir" $raw_arg "$script"
      rc=$?
      times >"$out_dir/.k6-times"
      exit "$rc"
    ) >&2
    status=$?
  else
    local raw_arg="" pin_arg="" net_arg=""
    [ "$raw" = 1 ] && raw_arg="--out json=/out/raw.ndjson.gz"
    [ "$pin" = 1 ] && pin_arg="--cpuset-cpus $cpus_k6 -e GOMAXPROCS=1"
    [ "$target" = kind ] && net_arg="--network kind"
    # shellcheck disable=SC2086
    docker run --rm --name "$cluster-k6" $net_arg $pin_arg -u "$(id -u):$(id -g)" \
      -v "$here:/k6bench:ro" -v "$out_dir:/out" -w /k6bench "$k6_image" \
      run $k6_env -e OUT_DIR=/out $raw_arg "$script" >&2
    status=$?
  fi
  set -e
  echo "$status"
}

# Cores a native k6 averaged over its run, from `times`' children line
# (`0m12.3s 0m1.2s`: user, system), or `null` when k6 ran in Docker.
k6_cpu_cores() {
  local wall="$1" file="$out_dir/.k6-times"
  if [ ! -f "$file" ] || [ "$wall" -le 0 ]; then
    echo null
    return
  fi
  awk -v wall="$wall" 'NR == 2 {
    cpu = 0
    for (i = 1; i <= 2; i++) { split($i, t, /[ms]/); cpu += t[1] * 60 + t[2] }
    printf "%.3f\n", cpu / wall
  }' "$file"
  rm -f "$file"
}

k6_version() {
  if [ "$1" = native ]; then k6 version | head -1; else docker run --rm "$k6_image" version | head -1; fi
}

# --- main -------------------------------------------------------------------

case "$cmd" in
  up) stack_up; exit 0 ;;
  down) stack_down; exit 0 ;;
esac

[ -f "$here/scenarios/$scenario.js" ] || die "unknown scenario $scenario (see scenarios/)"
# These reach k6's command line and metadata.json unquoted; in CI they come
# from free-text workflow inputs.
is_int() { case "$1" in '' | *[!0-9]*) return 1 ;; esac; }
is_duration() { printf '%s' "$1" | grep -Eq '^[0-9]+[smh]?$'; }
is_int "$rate" || die "--rate must be a whole number of requests/s: $rate"
is_int "$workloads" || die "--workloads must be a whole number: $workloads"
is_int "$slo_p99_ms" || die "--slo-p99-ms must be a whole number: $slo_p99_ms"
is_duration "$duration" || die "--duration must look like 90s, 3m or 1h: $duration"
is_duration "$warmup" || die "--warmup must look like 30s, 2m or 1h: $warmup"
[ -z "$stress_rates" ] || printf '%s' "$stress_rates" | grep -Eq '^[0-9]+(,[0-9]+)*$' ||
  die "--stress-rates must be comma-separated whole numbers: $stress_rates"
case "$target" in
  kind) ;;
  kube)
    [ -n "$target_url" ] || die "--target kube needs --target-url (how k6 reaches the host group)"
    [ -n "$registry" ] || die "--target kube needs --registry (reachable from the cluster)"
    ;;
  *) die "--target must be kind or kube" ;;
esac
case "$routing" in
  local | network) ;;
  *) die "--routing must be local or network" ;;
esac
case "$scenario" in
  fan-out-10 | chain-5) ;;
  *) [ "$routing" = local ] || die "--routing applies only to fan-out-10 and chain-5" ;;
esac

stamp="$(date -u +%Y-%m-%dT%H%M%SZ)"
suffix=""
[ "$routing" = network ] && suffix=_network
out_dir="${out_dir:-$repo/bench-results/${stamp}_${scenario}_${profile}${suffix}}"
mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
exec > >(tee -a "$out_dir/run.log") 2>&1

wash_image=""
operator_image=""
if [ "$target" = kind ]; then
  stack_up
  # A native wash pushes through the host port; the wash image pushes from
  # inside the kind network. Either way the hosts pull by kind-network IP.
  if [ -n "${WASH:-}" ] || command -v wash >/dev/null 2>&1; then
    push_components "localhost:$registry_port" "$(registry_in_cluster)"
  else
    push_components "$registry_name:5000" "$(registry_in_cluster)"
  fi
else
  need kubectl
  kctl create namespace "$namespace" --dry-run=client -o yaml | kctl apply -f - >/dev/null
  push_components "$registry" "$registry"
fi

cleanup() {
  local pid
  for pid in ${sampler_pid:-} ${k6_sampler_pid:-}; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  if [ "$keep" = 0 ]; then
    log "removing $scenario workloads"
    manifests | kctl -n "$namespace" delete --ignore-not-found --wait=false -f - >/dev/null 2>&1 || true
  fi
  [ "$down" = 1 ] && [ "$target" = kind ] && stack_down
  return 0
}
trap cleanup EXIT

log "deploying $scenario workloads"
manifests >"$out_dir/manifests.yaml"
deploy_start="$(date +%s)"
kctl -n "$namespace" apply -f "$out_dir/manifests.yaml" >&2
kctl -n "$namespace" wait --for=condition=Ready workloaddeployment --all --timeout=10m >&2
sched_s=$(($(date +%s) - deploy_start))

read -r mode url <<<"$(k6_mode)"
host_header="$(default_host)"
wait_routable "$url" "$host_header" "$mode"
log "k6 ($mode) → $url  scenario=$scenario profile=$profile routing=$routing rate=$rate duration=$duration"

sampled=("$cluster-control-plane" "$cluster-worker")
[ "$mode" = docker ] && sampled+=("$cluster-k6")
if [ "$target" = kind ]; then
  sample_cluster "$out_dir/cluster.ndjson" "${sampled[@]}" &
  sampler_pid=$!
fi
if [ "$mode" = native ]; then
  sample_native_k6 "$out_dir/cluster.ndjson" "$cluster-k6" &
  k6_sampler_pid=$!
fi

k6_started="$(date +%s)"
k6_status="$(run_k6 "$mode" "$url" | tail -1)"
k6_ended="$(date +%s)"

# 0 = pass, 99 = thresholds breached (a result, not a harness failure).
case "$k6_status" in
  0 | 99) ;;
  *) die "k6 exited $k6_status" ;;
esac

cat >"$out_dir/metadata.json" <<EOF
{
  "schema": 1,
  "scenario": "$scenario",
  "profile": "$profile",
  "rate": $rate,
  "duration": "$duration",
  "warmup": "$warmup",
  "workloads": $workloads,
  "routing": "$routing",
  "target": "$target",
  "target_url": "$url",
  "k6_mode": "$mode",
  "k6_version": "$(k6_version "$mode")",
  "k6_exit": $k6_status,
  "k6_started": $k6_started,
  "k6_ended": $k6_ended,
  "pinned": $([ "$pin" = 1 ] && echo true || echo false),
  "k6_cpu_cores": $(k6_cpu_cores $((k6_ended - k6_started))),
  "wash_image": "$wash_image",
  "operator_image": "$operator_image",
  "components": ["$image_hello", "$image_relay"],
  "deploy_ready_s": $sched_s,
  "git_sha": "$(git -C "$repo" rev-parse HEAD)",
  "git_dirty": $([ -n "$(git -C "$repo" status --porcelain)" ] && echo true || echo false)
}
EOF

log "results in $out_dir"
if [ "$ci" = 0 ]; then
  (cd "$repo" && cargo run -p bench-tools --quiet -- k6 report "$out_dir") || true
fi
