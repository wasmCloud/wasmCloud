# Local Ingress (same-host routing)

Two components demonstrating **same-host local routing**: when a caller and a
callee are scheduled onto the same wasmCloud host and the host runs with
`--http-local-routing`, the caller's `wasi:http` outgoing request is served by
the callee **in-memory** — it never touches the network, kube-proxy, or any
ingress in between.

- [`caller/`](./caller/) — HTTP handler that makes an outgoing request to the
  callee and reports the result. Target resolution: `?url=` query param →
  `CALLEE_URL` env var → `http://callee.internal/hello`.
- [`callee/`](./callee/) — HTTP handler that answers with a greeting echoing
  the path and `Host` header it received.

Same-host routing takes **two keys**, and neither works alone:

1. The **host operator** enables the capability — `--http-local-routing`, or
   `runtime.hostGroups[].http.localRouting` in the chart.
2. The **workload** declares what it offers co-located callers, with
   `localRoute` on its `wasi:http/incoming-handler` interface config.

This mirrors how `allowedHostLoopback` works elsewhere in the runtime: a
workload author cannot open the door, and neither can an operator.

`localRoute` is comma-separated and takes two forms:

| Entry | Serves |
| --- | --- |
| `functiona.internal` | every path on that hostname |
| `functiona.internal/hello` | `/hello` and below, on that hostname |

The example uses `functiona.internal/hello`. That name resolves nowhere in real
DNS — which is the point: a 200 from the caller proves the request was
short-circuited on the host.

Three things worth knowing:

- **Prefixes match on `/` segment boundaries.** `/hello` covers `/hello` and
  `/hello/items`, but not `/hello-world`. A request to a path no route claims
  falls through to the network rather than being handed to a workload that does
  not serve it.
- **The two scopes do not overlap.** `host` and `host-aliases` publish to the
  network; `localRoute` offers in-memory. A hostname published only to the
  network is never short-circuited for a co-located caller, and a `localRoute`
  name is never reachable from the network — so nobody inside the cluster can
  reach `functiona.internal` by dialing the host pod with a forged `Host`
  header. Declare a name in both places if you want it reachable both ways.
- **Only the callee declares anything.** The caller needs no config beyond an
  `allowedHosts` entry permitting the authority, which is enforced *before*
  local routing is consulted.

> **Heads up:** locally routed calls bypass whatever sits on the network path
> (ingress auth, rate limits, mesh mTLS, NetworkPolicy). The feature is off by
> default and must be enabled per host; the caller's `allowedHosts` egress
> policy is still enforced first.

## Prerequisites

- `cargo` 1.85+ with the `wasm32-wasip2` target
- [`wash`](https://wasmcloud.com/docs/installation) 2.0

## Running with wash dev

`wash dev` runs one component per session, so the dev loop uses two sessions
talking over loopback (the caller's `.wash/config.yaml` sets
`CALLEE_URL=http://localhost:8001`). This exercises the same application code;
the in-memory short-circuit itself is a multi-workload host feature you can
see under "Trying local routing on a single host" below.

Terminal 1 — the callee on port 8001:

```shell
wash -C callee dev
```

Terminal 2 — the caller on port 8000:

```shell
wash -C caller dev
```

Then:

```shell
$ curl localhost:8000
caller -> http://localhost:8001
upstream status: 200 OK
upstream body: hello from the callee! (path: /, host: localhost:8001)
```

You can point the caller anywhere with the query override:
`curl 'localhost:8000/?url=http://localhost:8001/some/path'`.

## Trying local routing on a single host

To see the actual in-memory short-circuit outside Kubernetes, run one
`wash host` with local routing enabled and start both workloads on it (for
example via the runtime operator, or a scheduler pointed at the host):

```shell
wash host --http-addr 0.0.0.0:9191 --http-local-routing ...
```

With both workloads placed on that host — and the callee declaring
`localRoute: callee.internal/hello` — the caller's request to
`http://callee.internal/hello` succeeds even though
`callee.internal` has no DNS entry. Stop the host without
`--http-local-routing` and the same call fails with a connection error — the
flag is the only thing serving it.

## Deploying to a kind cluster

Assumes the [runtime-operator chart](../../charts/runtime-operator/) manages
your cluster.

### 1. Enable local routing on the hostgroup

Local routing is a host-level opt-in. Set it on the hostgroup that will run
both workloads (`values.yaml`):

```yaml
runtime:
  hostGroups:
    - name: default
      replicas: 1 # both workloads must land on the SAME host
      http:
        enabled: true
        port: 9191
        localRouting: true
```

```shell
helm upgrade --install wasmcloud ./charts/runtime-operator -f your-values.yaml
```

With `replicas: 1` co-location is guaranteed. With more host replicas, the
scheduler may place the workloads on different hosts, in which case the call
falls back to the network path (and fails for `functiona.internal`) — use a
dedicated hostgroup if you need determinism at scale.

### 2. Build and push the components (optional)

The manifest references `ghcr.io/wasmcloud/components/local-ingress-{caller,callee}`.
To use your own registry:

```shell
wash -C callee build && wash oci push <your-registry>/local-ingress-callee:0.1.0 callee/target/wasm32-wasip2/release/local_ingress_callee.wasm
wash -C caller build && wash oci push <your-registry>/local-ingress-caller:0.1.0 caller/target/wasm32-wasip2/release/local_ingress_caller.wasm
```

…and update the `image:` fields in [`deploy/workloads.yaml`](./deploy/workloads.yaml).


### 3. Setup Kind cluster

#### 3a. Install Traefik as the ingress (NodePort 30950)

Traefik listens on NodePort **30950**, which the kind config exposes as host
port 80. Every `*.localhost.cosmonic.sh` request then flows: laptop `:80` → node
`:30950` → Traefik → the `Ingress` matched by Host header.

```bash
helm repo add traefik https://traefik.github.io/charts
helm repo update traefik

helm upgrade --install traefik traefik/traefik \
  -n traefik --create-namespace \
  --set service.type=NodePort \
  --set ports.web.nodePort=30950
```

#### 3b. load local wash build
```bash
kind load docker-image wash:local-ingress
```


#### 3c. install wasmCloud with local build
```bash
helm install wasmcloud --version 2.5.2 oci://ghcr.io/wasmcloud/charts/runtime-operator \
  --namespace wasmcloud --create-namespace -f ./deploy/values.local-ingress.yaml
```

#### 3d. Deploy an in-cluster OCI registry (optional)

[`deploy/oci-registry.yaml`](./deploy/oci-registry.yaml) runs the
[examples/oci-registry](../oci-registry/) component as a WorkloadDeployment so
you have a place to `wash oci push` your locally built components, instead of
publishing to an external registry. It bundles:

- a `Service` (`oci-registry`) whose EndpointSlice the operator manages,
- the `WorkloadDeployment` itself (wasip3 registry backed by the host's
  filesystem blobstore under `/tmp/oci-registry` — survives workload restarts,
  not pod restarts),
- a Traefik `Ingress` for `registry.localhost.cosmonic.sh`, so pushes from
  your laptop flow `:80` → node `:30950` → Traefik → the registry.

The registry speaks plain HTTP: laptop-side pushes need `wash oci push
--insecure`, and the hosts pull from it thanks to
`--allow-insecure-registries`, which
[`deploy/values.local-ingress.yaml`](./deploy/values.local-ingress.yaml)
already sets via the hostgroup's `extraArgs`.

> **Note:** the registry component imports the async
> `wasmcloud:blobstore@0.1.0` interface, so the `wash:local-ingress` host
> image must be built with the `wasm_component_model_implements` feature.

```shell
kubectl apply -f deploy/oci-registry.yaml
kubectl get workloaddeployment oci-registry

# Sanity check from the laptop (OCI distribution API version endpoint):
curl -i http://registry.localhost.cosmonic.sh/v2/
```

Push the components built in step 2 to it:

```shell
wash oci push --insecure registry.localhost.cosmonic.sh/local-ingress-callee:0.1.0 callee/target/wasm32-wasip2/release/local_ingress_callee.wasm
wash oci push --insecure registry.localhost.cosmonic.sh/local-ingress-caller:0.1.0 caller/target/wasm32-wasip2/release/local_ingress_caller.wasm

# List what's in the registry:
curl -s http://registry.localhost.cosmonic.sh/v2/local-ingress-callee/tags/list
```

Then point the `image:` fields in
[`deploy/workloads.yaml`](./deploy/workloads.yaml) at the registry's
**in-cluster** name — the hosts can't use `registry.localhost.cosmonic.sh`
(it resolves to `127.0.0.1`), but the operator registers the Service DNS name
with the router, so pulls go through the Service:

```yaml
image: oci-registry.default.svc:80/local-ingress-callee:0.1.0
```

(Adjust `default` if you applied the registry manifest into another
namespace. The repository path and tag are exactly what you pushed; only the
registry hostname differs between push and pull.)

Keep the explicit `:80` — it is the port the registry Service publishes, and
an image reference carries no scheme, so without it the pull dials the default
port and finds nothing listening.

> **Note**
> `--allow-insecure-registries` (set in `deploy/values.local-ingress.yaml`)
> switches *every* registry this host pulls from to plain HTTP, not just the
> in-cluster one. That is acceptable for a local kind cluster and nothing else.
> The supported alternative is to serve the registry over TLS and have the
> hosts trust its CA by path (`runtime.ociCaPaths`, and `wash oci push
> --ca-path` on the push side), which is what the e2e suite does. Porting this
> example onto that is tracked separately.

### 4. Deploy the workloads

```shell
kubectl apply -f deploy/workloads.yaml
kubectl get workloaddeployments
```

### 5. Test

The manifest includes a Service (`local-ingress-caller`, EndpointSlice managed
by the operator) and a Traefik Ingress for `hello.localhost.cosmonic.sh`, so
the caller is directly reachable from the laptop — no port-forward needed:

```shell
$ curl http://hello.localhost.cosmonic.sh/
caller -> http://functiona.internal/hello
upstream status: 200 OK
upstream body: hello from the callee! (path: /hello, host: functiona.internal)
```

The echoed `host: functiona.internal` is the proof: no DNS record or Service
exists for that name, so the only way the request reached the callee is the
host's in-memory local route. The echoed `path: /hello` is the prefix the
callee's `localRoute` is scoped to.

Three things worth reproducing, because each isolates one property:

```shell
# Wrong path — the authority still matches, but no route claims `/`, so the
# request falls through to the network and there is nothing there.
$ curl 'http://hello.localhost.cosmonic.sh/?url=http://functiona.internal/'
caller -> http://functiona.internal/
request failed: ...

# Wrong host — `/hello` is right, but nothing offers this name locally.
$ curl 'http://hello.localhost.cosmonic.sh/?url=http://nope.internal/hello'
caller -> http://nope.internal/hello
request failed: ...

# The localRoute name is not reachable from the network: it exists only in the
# host's in-memory table, so a forged Host header gets a 404, not the callee.
$ curl -H 'Host: functiona.internal' http://localhost/hello
404 page not found
```

(The second is also denied by the caller's `allowedHosts`, which lists only
`functiona.internal` — policy is checked before local routing, so a co-located
callee can never widen a caller's egress.)

To see the feature itself turned off, set `localRouting: false`, upgrade the
release, and the original curl reports `request failed` too.

## Required Capabilities

- `wasi:http/incoming-handler` (both) — to receive HTTP requests
- `wasi:http/outgoing-handler` (caller) — to call the callee
