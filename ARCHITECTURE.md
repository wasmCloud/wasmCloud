# wasmCloud Architecture

This document describes the parts of wasmCloud, who acts on them, and what happens when a workload is deployed, serves a request, calls out, and is released. The [self-assessment](SELF_ASSESSMENT.md) and [threat model](THREAT_MODEL.md) cover the same system from a security point of view: trust boundaries, goals, and what counts as a vulnerability. External interfaces are listed in [API.md](API.md).

## Parts

| Part | Source | What it does |
| --- | --- | --- |
| Component | Built by users with `wash build` | A WebAssembly component. It can call only the interfaces it imports and the host links. |
| Workload | A `Workload` resource or a spec sent over NATS | A set of components, plus at most one long-running service component, deployed together. The spec lists what the workload is granted: interfaces, allowed hosts, volumes, configuration and secrets. |
| Host | [`crates/wash-runtime`](crates/wash-runtime/), run by `wash host` or `wash dev` | Compiles and runs components with wasmtime, links the interfaces each workload is granted, and enforces its limits. |
| Plugin | Native Rust in the host, or a Wasm host component | Implements an interface (key-value, blobstore, messaging, secrets, and others) for the workloads that bind it. |
| `wash` | [`crates/wash`](crates/wash/) | The CLI. Scaffolds, builds and publishes components, runs a development host (`wash dev`), and runs a cluster host (`wash host`). |
| `runtime-operator` | [`runtime-operator/`](runtime-operator/) | A Kubernetes operator. Reconciles the wasmCloud CRDs, picks a host for each workload, and sends it the spec over NATS. Routes HTTP through EndpointSlices on standard Services. |
| `runtime-gateway` | [`runtime-gateway/`](runtime-gateway/) | A deprecated HTTP proxy to hosts, kept for existing installs. |
| NATS | External | The control-plane bus between the operator and hosts. Messages are defined in [`proto/`](proto/). |
| OCI registry | External | Stores components. Hosts pull from it; `wash oci` pushes to it. |

## Actors

| Actor | Acts through | Trusted to |
| --- | --- | --- |
| Component author | `wash`, an OCI registry | Write application code. Their code is untrusted and sandboxed. |
| Platform operator | Helm, `kubectl`, host flags | Install and configure hosts, NATS, plugins and policy. |
| Workload author | `Workload` resources, or specs on NATS | Decide what a workload is granted. The host trusts the spec. |
| Client | HTTP, NATS subjects a workload subscribes to | Send requests. Their input is untrusted. |
| Maintainer | GitHub, the release workflows | Review, merge and release. |

## Actions

### Deploying a workload

1. A workload author applies a `Workload`, `WorkloadDeployment`, `WorkloadReplicaSet` or `WorkloadDaemonSet`.
2. `runtime-operator` picks a ready host, from the heartbeats hosts publish, and sends it the workload spec over NATS.
3. The host pulls each component from its registry, validates and compiles it, and links only the interfaces the spec grants.
4. The host applies the spec's allowed hosts, volumes, configuration, secrets and limits, then starts the service component, if there is one, and reports the result.
5. The operator records the result in the resource's status, and adds the host to the EndpointSlices of any Service that routes to the workload.

`wash dev` runs steps 3 and 4 in-process from a local project, with no operator or NATS.

### Serving a request

1. An HTTP request reaches a host through a Kubernetes Service, or through `runtime-gateway` on installs that still use it.
2. The host matches the request's hostname to a workload, takes an instance of its component, and calls the exported `wasi:http` handler.
3. A message on a NATS subject a workload subscribes to is delivered the same way, through the `wasmcloud:messaging` handler.

### Calling out

- **HTTP and sockets.** The host checks each outbound HTTP destination against the workload's allowed hosts, and each socket address against the allowed hosts and the host's address policy.
- **Plugins.** When a component calls an interface a plugin implements, the host passes the plugin the calling workload's identity.
- **Linked components.** Components in one workload call each other's exports directly, inside the host.

### Releasing

A release train opens a version-bump pull request every two weeks. After a maintainer approves it and CI passes, the tag is pushed, images that passed CI are promoted without a rebuild, and binaries, images and WIT packages are published with signed provenance. See [RELEASE_RUNBOOK.md](RELEASE_RUNBOOK.md).

## Further reading

- [wasmcloud.com/docs/runtime](https://wasmcloud.com/docs/runtime): the host and its plugin model
- [wasmcloud.com/docs/kubernetes-operator](https://wasmcloud.com/docs/kubernetes-operator): the operator and its CRDs
- [runtime-operator/README.md](runtime-operator/README.md): NATS connections and permissions
