# k6bench: load-testing wasmCloud with k6

System-level load tests for wasmCloud v2. [k6](https://k6.io) drives HTTP
traffic at workloads that are scheduled by the runtime-operator onto wash hosts
in a Kubernetes cluster. The same `run.sh` works on a laptop, in CI on the
bench host, and against your own cluster.

This is not the same as the criterion and gungraun benches in
[`scripts/bench`](../bench/README.md). Those measure `wash-runtime` in-process
and gate regressions commit by commit. k6bench measures a deployed stack:

- the throughput and tail latency it sustains
- where it breaks
- what the traffic costs in CPU and memory

## Quickstart (laptop)

Prerequisites:

- Docker
- [kind](https://kind.sigs.k8s.io)
- kubectl
- helm
- Rust with the `wasm32-wasip2` target (the repo's `rust-toolchain.toml` installs it)

`k6` and `wash` are optional. Without them, `run.sh` uses the
`grafana/k6:2.3.0` and `ghcr.io/wasmcloud/wash` images. Building the bench
components compiles `wash` from the tree once, the first time
`cargo xtask build-fixtures` runs.

```bash
# One scenario: creates the kind cluster on first use, installs the chart,
# builds and deploys the bench components, runs k6, prints the report.
./scripts/k6bench/run.sh --scenario http-hello --rate 1000 --duration 2m

# Find the knee: step the offered rate up until p99 or errors break the SLO.
./scripts/k6bench/run.sh --scenario http-hello --profile stress

# Compare two runs of the same scenario/profile/rate.
./scripts/k6bench/compare.sh bench-results/<baseline> bench-results/<candidate>

# Done for the day.
./scripts/k6bench/run.sh down
```

The cluster is kept between runs, and each run re-applies the chart. Each run
removes its workloads when it finishes. Two flags change that:

- `--keep` leaves the workloads up for poking at with
  `curl -H 'Host: hello.k6bench' http://127.0.0.1:30950/`. Delete them before
  running another scenario, because each scenario's entry Service claims
  NodePort 30950.
- `--reuse-stack` skips the chart install and image builds, so iterating on a
  scenario takes seconds.

By default the chart runs the released images for its `appVersion`. If those
aren't on ghcr.io yet (a release commit lands before its images do), it runs the
latest GitHub release instead. Two flags change that:

- `--wasmcloud-version X.Y.Z` runs another release.
- `--build-local` builds `wash` and `runtime-operator` from the working tree and
  loads them into kind.

Laptop numbers swing 10–20% between identical runs. They are for developing
scenarios, not for publishing.

## Scenarios

| Scenario         | Deployed                                              | What it isolates                                               |
| ---------------- | ----------------------------------------------------- | -------------------------------------------------------------- |
| `http-hello`     | one `hello` workload                                  | the platform's per-request cost; counterpart of `http_invoke`  |
| `fan-out-10`     | a `relay` calling 10 `hello`s concurrently            | inter-workload call graph                                      |
| `chain-5`        | five `relay`s in a line                               | per-hop overhead: (chain p50 − hello p50) / 4                  |
| `many-workloads` | `--workloads N` `hello`s, requests rotate Host header | routing and per-workload cost as the count grows               |

Each scenario is a k6 script in [`scenarios/`](scenarios/). Its
WorkloadDeployments are in [`manifests/`](manifests/). A `*.each.yaml` file is
rendered once per instance with `__I__` as the index. The components are
`wash-runtime` test fixtures in
[`crates/wash-runtime/tests/fixtures/`](../../crates/wash-runtime/tests/fixtures/),
built with `cargo xtask build-fixtures`:

- `hello` is `http-handler-p2`, the fixture behind the `http_invoke` criterion
  bench. It returns a static 200.
- `relay` is `http-relay`. It calls every URL in its `TARGETS` config and returns
  200 only if all of them did.

### Local routing

`fan-out-10` and `chain-5` make calls between workloads. `--routing` picks how
those calls travel:

- `local` (default): the relays call their targets by `localRoute` name
  (`*.internal`), and the host's local routing (`http.localBypassRouting`) is
  on. The same host serves the call in memory.
- `network`: the relays call `<service>.<namespace>`, and the host's local
  routing is off. Each call goes out through cluster DNS, the Service and the
  node's network stack, then back into the host's ingress.

Run both at the same load and compare them. The difference is what local
routing saves:

```bash
./scripts/k6bench/run.sh --scenario chain-5 --routing local   --rate 500 --out-dir bench-results/chain-local
./scripts/k6bench/run.sh --scenario chain-5 --routing network --rate 500 --out-dir bench-results/chain-network
./scripts/k6bench/compare.sh bench-results/chain-local bench-results/chain-network
```

Switching modes reinstalls the chart and restarts the hosts, so
`--reuse-stack` refuses a run whose mode doesn't match the installed hosts. A
`network` run's history row has `-network` appended to its `param`, and its
default result directory ends in `_network`.

`run.sh` pushes each component to the local registry, tagged by its content
hash. The host caches images by tag, so reusing a tag would keep serving the
old build.

Each manifest pairs every WorkloadDeployment with its own Service through
`spec.kubernetes.service.name`. The Service has no selector: the operator
writes its EndpointSlice (the host pods running the workload) and registers
`<service>.<namespace>` as a Host alias for it.

- The scenario's entry Service is a NodePort on 30950, which is where k6 sends
  requests.
- The other Services are ClusterIP.
- `many-workloads` sends every Host header through `hello-0`'s NodePort,
  because the host routes by Host header, not by Service.

`kubectl port-forward svc/<name>` doesn't work for these Services: it follows
a pod selector, and they have none. Use the NodePort, or port-forward the host
pod itself.

## Profiles

| Profile    | Load                                                            | Headline                                      |
| ---------- | --------------------------------------------------------------- | --------------------------------------------- |
| `constant` | `--rate` req/s for `--duration`                                 | rps, p50/p90/p95/p99/p99.9, error rate        |
| `stress`   | steps of `--stress-rates` (default 500 … 15000), 30 s each      | `max_sustainable_rps`, `knee_offered_rps`     |
| `spike`    | `--rate`, then 10× that, then `--rate` again, `--duration` each | peak and post-spike p99 and error rate        |

Every profile starts with an unreported warm-up (`--warmup`, 30 s). All
executors are open-model (`constant-arrival-rate`). k6 keeps sending at the
offered rate even when responses slow down, so a slow system can't hide its
own latency (no coordinated omission).

The SLO is `--slo-p99-ms`, 250 ms by default. A window holds the SLO when p99
is within it, errors stay under 1%, and k6 drops fewer than 0.1% of the
iterations it should have started. `max_sustainable_rps` is the best rps
reached before the first stress step that breaks the SLO.

## Output

Each run writes `bench-results/<utc>_<scenario>_<profile>/`
(git-ignored), or `--out-dir`:

| File             | Contents                                                                   |
| ---------------- | -------------------------------------------------------------------------- |
| `summary.json`   | k6's `handleSummary` data plus each scenario's offered rate and length     |
| `metadata.json`  | what ran: scenario, profile, routing, images, k6 version and mode, git sha |
| `cluster.ndjson` | ~1 s `docker stats` samples of the kind nodes and a dockerized k6          |
| `manifests.yaml` | the rendered WorkloadDeployments                                           |
| `run.log`        | everything run.sh and k6 printed                                           |
| `raw.ndjson.gz`  | with `--raw`: k6's per-request stream (large)                              |

`cargo run -p bench-tools -- k6 report <dir>` renders the report. Add
`--markdown` for GitHub. `k6 jsonl <dir>` emits the `history.json` rows that
CI publishes, and `k6 delta <a> <b>` is what `compare.sh` runs.

**Generator-saturated runs.** A pinned run where k6 averaged over 90% of its
core is marked `generator_saturated`: k6 set the ceiling, not wasmCloud. It
still gets a report, but CI keeps it off the dashboard. k6's CPU comes from
`docker stats` for a dockerized k6 and from the shell's `times` for a native
one. Dropped iterations don't count here, because k6 also drops them when a
slow system ties up every VU. They fail the SLO instead, and that result is
published.

## Running against your own cluster

To run against a cluster where wasmCloud is already installed:

```bash
./scripts/k6bench/run.sh --target kube \
  --registry registry.example.com/bench \
  --target-url http://<host-group-ingress> \
  --scenario http-hello --rate 2000
```

- `--target kube` uses the current kubectl context. It never creates or
  deletes a cluster, and it deploys only the bench workloads into
  `--namespace` (default `k6bench`).
- `--registry` is where the bench components are pushed. The hosts must be
  able to pull from it.
- `--target-url` is how k6 reaches the scenario's entry Service. The manifests
  make it a NodePort on 30950, so `http://<any-node>:30950` works. Behind a
  LoadBalancer or Ingress, point this at that address instead.
- `fan-out-10` and `chain-5` with `--routing local` need
  `http.localBypassRouting: true` on the host group. `--routing network` needs
  it off, or nothing differs but the names. `run.sh` doesn't change a cluster
  it didn't install.

The k6 scripts also run on their own, without `run.sh`, against anything
already deployed:

```bash
k6 run -e TARGET_URL=http://localhost:30950 -e RATE=500 scripts/k6bench/scenarios/http-hello.js
```

The environment variables they read are listed in [`lib/config.js`](lib/config.js).

## CI

[`.github/workflows/k6bench.yml`](../../.github/workflows/k6bench.yml) runs on
the Hetzner bench host, on `release: published` and on demand
(`workflow_dispatch`):

- **Release:** runs the release set against the images published for that
  tag: `http-hello` constant and stress, `fan-out-10` and `chain-5` with
  local and with network routing, and `many-workloads`.
- **Dispatch:** runs one scenario, or the release set, against any ref. For
  `fan-out-10` and `chain-5`, the `routing` input picks `local`, `network` or
  `both`. A ref that isn't a release tag is built with `--build-local`.

Each run:

- gets a fresh kind cluster
- writes a markdown step summary
- uploads a 90-day artifact
- prepares a data artifact for a GitHub-hosted publisher, which updates the
  same S3 layout and `history.json` as the criterion benches
  (`bench: "k6"`, `group: <scenario>`, `param: <profile>-<rate>`)
- ends by deleting the cluster and stopping the socket-activated Docker daemon,
  so the criterion and gungraun benches, whose pre-flight refuses a running
  daemon, get the host as they expect it

As in `bench.yml`, a pull-request ref never reaches S3.

On the bench host, `--ci` implies `--pin`, which applies this CPU layout:

| CPU          | Runs                                                                       |
| ------------ | -------------------------------------------------------------------------- |
| 0            | kind control-plane node (API server, etcd; tainted, so no chart pods)      |
| 1–4          | kind worker node: wash hosts, NATS, operator                               |
| 5 (isolated) | k6, via `taskset` (native) or `--cpuset-cpus` (docker), `GOMAXPROCS=1`     |

Override the layout with `K6BENCH_CPUS_CONTROL_PLANE`, `K6BENCH_CPUS_HOSTS` and
`K6BENCH_CPUS_K6`. On the bench host, k6 runs natively and targets the worker
node's IP directly, which bypasses Docker's port forwarding.

[`.github/workflows/k6bench-smoke.yml`](../../.github/workflows/k6bench-smoke.yml)
runs a 20-second `http-hello` on a GitHub-hosted runner for every PR that
touches this directory or `bench-tools`.

## Nested local routing (fixed in this tree)

Through 2.10.1, `chain-5` fails 4–12% of requests, even at one request at a
time. The same components over the network, through Service DNS, fail none.

The cause was in the host's same-host local routing. A caller writes its
outgoing body into a channel, and the receiving end went straight to the
callee. A callee that answered without reading the body dropped that receiver,
which closed the caller's stream. The caller's next `check_write` or `finish`
then failed with `StreamError::Closed`, which the guest sees as
`connection reset`.

`DrainOnDrop` in `crates/wash-runtime/src/host/http.rs` now drains an unread
body the way a network connection would. The bench components deliberately
don't read their request bodies, so `chain-5` against an older release shows
the bug.
