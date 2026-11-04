# wasmCloud External Interfaces

These are the interfaces wasmCloud exposes to users, operators and other software. Each row links the source of truth. How the parts fit together is in [ARCHITECTURE.md](ARCHITECTURE.md).

## For component authors

| Interface | Reference |
| --- | --- |
| WASI interfaces the host implements (`wasi:http`, `wasi:sockets`, `wasi:filesystem`, `wasi:config`, `wasi:keyvalue`, `wasi:blobstore`, `wasi:logging` and the rest of WASI) | [README: The platform](README.md#the-platform), [WASI](https://wasi.dev/) |
| `wasmcloud:messaging` | [`wit/messaging`](wit/messaging/wit/) |
| `wasmcloud:keyvalue` | [`wit/keyvalue`](wit/keyvalue/wit/) |
| `wasmcloud:blobstore` | [`wit/blobstore`](wit/blobstore/wit/) |
| `wasmcloud:nats` | [`wit/nats`](wit/nats/wit/) |
| `wasmcloud:postgres` | [`wit/postgres`](wit/postgres/wit/) |
| `wasmcloud:secrets` | [`wit/secrets`](wit/secrets/wit/) |

## For plugin authors and embedders

| Interface | Reference |
| --- | --- |
| `wasmcloud:host`, the interfaces a Wasm host component plugin imports and exports (workload identity, lifecycle, secret sources) | [`wit/host`](wit/host/wit/) |
| The `wash-runtime` Rust API for custom hosts and native plugins | [docs.rs/wash-runtime](https://docs.rs/wash-runtime), [Building custom hosts](https://wasmcloud.com/docs/runtime/building-custom-hosts), [Creating host plugins](https://wasmcloud.com/docs/runtime/creating-host-plugins) |

## For platform operators

| Interface | Reference |
| --- | --- |
| `wash` CLI commands and flags | [README: `wash` commands](README.md#wash-commands), [wasmcloud.com/docs/wash/commands](https://wasmcloud.com/docs/wash/commands), `wash help <command>` |
| `wash host` flags and environment variables | `wash host --help`, [`crates/wash/src/cli/host.rs`](crates/wash/src/cli/host.rs) |
| Kubernetes CRDs (`Host`, `Workload`, `WorkloadDeployment`, `WorkloadReplicaSet`, `WorkloadDaemonSet`, `Artifact`) | [CRD schemas](runtime-operator/config/crd/bases/), [wasmcloud.com/docs/kubernetes-operator/crds](https://wasmcloud.com/docs/kubernetes-operator/crds) |
| Helm chart values | [`charts/runtime-operator/values.yaml`](charts/runtime-operator/values.yaml) |
| `runtime-operator` flags and NATS permissions | [runtime-operator/README.md](runtime-operator/README.md) |

## Network interfaces

| Interface | Served by | Reference |
| --- | --- | --- |
| HTTP ingress to workloads, optionally TLS or mutual TLS | Host, `--http-addr` | `wash host --help` |
| `/livez` and `/readyz` probes | Host, `--probe-addr` | `wash host --help` |
| Control-plane messages on `runtime.host.>` and `runtime.operator.heartbeat.>` | Host and operator, over NATS | [Protobuf definitions](proto/wasmcloud/runtime/v2/), [subject permissions](runtime-operator/README.md#permissions) |
| HTTP proxy to hosts (deprecated) | `runtime-gateway` | [runtime-gateway/README.md](runtime-gateway/README.md) |
