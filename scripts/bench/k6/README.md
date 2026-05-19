# wasmCloud k6 benches

End-to-end load benches that run k6 against the runtime-operator deployed
in a single-node k3s cluster on the bench host. Results are surfaced on
[arewefastyet](https://wasmcloud.github.io/arewefastyet) the same way
the criterion / iai-callgrind benches are: each row in `history.json`
carries a `metric` (e.g. `req_per_s`, `p95_ms`, `error_rate`) and a
`value`, plus the standard provenance (`sha`, `ref`, `host`, …).

> **TL;DR.** k3s instead of kind (no Docker layer between k6 and the
> wash host). The cluster is a long-lived systemd service; each bench
> reinstalls the chart and reapplies the workload, runs k6, then tears
> the workload back down. k6's JSON summary is parsed by `bench-tools`
> into the same JSONL row schema everything else uses.

## Why k3s, not kind

| | k3s | kind |
|---|---|---|
| Host overhead | systemd unit, native binary | Docker → containerd → kubelet |
| Reproducibility | One layer between k6 and the host pod | Two (kind container + host pod container) |
| Cluster startup | ~5 s | ~30 s |
| Already used in repo | no | yes (operator e2e) |

For benchmarking, k3s wins: less variance, less startup, no Docker-in-Docker
masking the host-pod's actual resource use. For *operator e2e* (where the
goal is correctness of `helm install` against a stock cluster), kind stays.
The two paths share the chart, the operator image, and the runtime image
— only the cluster layer differs.

## What gets measured

Each bench is an end-to-end HTTP throughput / latency test against a
`WorkloadDeployment` running on a wash host pod. The value-add over the
in-process `http_invoke` criterion bench is:

- exercises the full kube-side hot path (Service → host Pod → wash → wasm)
- gives latency *percentiles*, not just mean ± CI
- catches regressions in operator routing / `EndpointSlice` plumbing

Variance is higher than criterion's: the wash host pod is *not* pinned
to the isolated CPU (k3s schedules its own workloads on the general
scheduler set, and pinning the kubelet-managed pod would race kube's
own resource manager). The bench host's other knobs — `nosmt`,
`scaling_governor=performance`, no resync — still apply, so run-to-run
noise is bounded; we just don't claim ns-level precision here.

## Bench types

| name | scenario | what it stresses |
|---|---|---|
| `k6_http_invoke` | constant-RPS hello-world for 30 s | steady-state throughput + tail latency |

More benches (ramp/spike, multi-component, large payload) are planned;
the runtime is in place. Add by dropping a `*.js` next to `http_invoke.js`
and listing it in `bench.yml`'s `workflow_dispatch.choice`.

## JSONL schema

`bench-tools jsonl --bench k6_http_invoke` emits one row per
`(scenario, stage, metric)` with the same envelope criterion / iai use:

```json
{
  "bench": "k6_http_invoke",
  "group": "constant_rps",
  "param": "200rps",
  "metric": "p95_ms",
  "value": 2.4,
  "sha": "…", "short_sha": "…", "ref": "…",
  "run_id": "…", "run_attempt": "…", "timestamp": "…",
  "host": "…", "kernel": "…", "cpus_online": 6, "isolated_cpu": "5"
}
```

Metrics emitted per `(scenario, stage)`:

| metric | unit |
|---|---|
| `req_per_s` | requests / second |
| `p50_ms` | milliseconds |
| `p95_ms` | milliseconds |
| `p99_ms` | milliseconds |
| `error_rate` | 0..1 |

The dedup key in `bench-push-results.mjs` already includes `metric`, so
all five rows land in `history.json` without colliding with each other
or with criterion's `mean_ns` rows for the same `(group, param)`.

## Cluster lifecycle

```text
                       ┌─────────────────────────┐
                       │  k3s.service (always on) │
                       └─────────────────────────┘
                                 │
   run-k6-bench.sh ──┬─→ helm upgrade --install operator-bench charts/runtime-operator
                     │     (uses values.bench.yaml)
                     ├─→ kubectl apply -f workloads/hello.yaml
                     ├─→ kubectl rollout status …
                     ├─→ k6 run --summary-export=summary.json scripts/bench/k6/<name>.js
                     ├─→ cargo run -p bench-tools -- summary --bench k6_<name>
                     └─→ helm uninstall + kubectl delete (workload + namespace)
```

The cluster itself stays up — `helm uninstall` clears state but keeping
the container runtime warm shaves ~20 s/run off image pulls. The
`WASMCLOUD_BENCH_K6_RESET_CLUSTER=1` env var forces `k3s-killall.sh +
systemctl restart k3s` before the run when something has corrupted
cluster state.

## Files in this directory

- `README.md` — this file
- `http_invoke.js` — k6 script: constant-RPS, hello-world component
- `workloads/hello.yaml` — `WorkloadDeployment` + `Service` deployed
  before each k6 run
- `values.bench.yaml` — Helm values for the bench-time install (single
  host group, ClusterIP, no TLS, debug logging off)

## Operator runbook

Provisioning, kernel tweaks, and the GHA pipeline are all documented in
[scripts/bench/README.md](../README.md). The k6-specific delta is:

- §5 (Toolchain) installs `k3s`, `kubectl`, `helm`, and `k6`.
- §9 (Running a bench) covers `k6_*` benches alongside `http_invoke` /
  `iai_callgrind`.
- §11 (What gets stored where) documents `k6.tar.zst` next to
  `criterion.tar.zst` / `iai.tar.zst` in the per-run S3 prefix.
