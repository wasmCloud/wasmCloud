# wasmCloud Threat Model

This document covers this repository, laying out what wasmCloud defends against, it's trust model, and what we treat as a security vulnerability.

To report a vulnerability, follow [SECURITY.md](SECURITY.md).

## Scope

In scope:

- The host: `wash host` and the `wash-runtime` crate, including the plugins it ships.
- `runtime-operator`, `runtime-gateway`, and the Helm charts in `charts/`.
- Release builds with default features, on the platforms we publish.

Out of scope:

- `wash dev` and other developer-focused workflows which run code developer-controlled code on the developer's own machine.
- Features marked experimental, and non-default build features.
- End-user applications deployed onto wasmCloud.

## Actors and trust

| Actor | Trust | Why |
| --- | --- | --- |
| Operator | Trusted | Runs the cluster and the hosts, and sets host flags and chart values. Anything an operator can configure is not an attack. |
| Control plane | Trusted | `runtime-operator`, and anyone who can publish on the host's `runtime.host.>` NATS subjects. Either can start any workload with any grant. |
| Workload spec author | Trusted to grant | A workload's spec (allowed hosts, volumes, config, secrets, interfaces) is a set of grants. The host cannot tell who wrote it. Host flags bound what any spec can grant. |
| Plugin author | Trusted | A native plugin is host code. A Wasm host component plugin is sandboxed, but it serves every workload that binds it. |
| Component code | **Untrusted** | The primary adversary. We assume it is hostile from its first instruction. |
| Network peers | **Untrusted** | Inbound HTTP clients, egress responses, and message payloads. |
| OCI artifacts | **Untrusted content** | Components are sandboxed. Their provenance (who published them) is the operator's concern; the host does not verify signatures. |

If your platform lets tenants write workload specs, the spec author is untrusted too. The runtime does not decide who may author a spec, so constrain specs before they reach a host, with Kubernetes RBAC and admission policy.

## Trust boundaries

```
 operator / control plane (trusted)
        │  workload specs, host flags
        ▼
 ┌──────────── host process (trusted) ─────────────────────────┐
 │  wasmtime + wash-runtime + native plugins                   │
 │                                                             │
 │   ┌─ workload A ─┐  ✗  ┌─ workload B ─┐   ┌─ Wasm plugin ─┐ │
 │   │ untrusted    │     │ untrusted    │──▶│ serves both   │ │
 │   └──────────────┘     └──────────────┘   └───────────────┘ │
 └───────┬────────────────────────────────────────┬────────────┘
         │ ingress (via runtime-gateway)          │ egress
     untrusted clients                       network / cluster
```

### 1. Component ↔ host

The wasmtime sandbox, plus the interfaces the host links into it. A component holds no ambient authority; it has only the imports its spec grants.

- **Malformed binaries.** A component arrives as untrusted bytes. The host validates, compiles and instantiates it, and may refuse it, but no binary may crash the host or corrupt its memory.
- **Resource use.** A component consumes CPU, memory, connections, file descriptors, instance slots and scratch disk. Memory, connections and file descriptors have per-workload and host-wide limits; CPU, instance slots and scratch size do not yet (see [Known limitations](#known-limitations)).

### 2. Workload ↔ workload

Each workload has its own stores, memory and virtual network: `127.0.0.1` inside a workload reaches only that workload. Workloads interact only through interfaces the host links between them.

### 3. Workload ↔ plugin

The host asserts the caller's workload identity to the plugin, and a guest cannot set or forge it. A plugin must authorize on that identity, never on an identity passed in a call's arguments.

### 4. Workload ↔ network (through the host)

Egress a workload originates is limited by its `allowedHosts` and by the host's address policy. Ingress reaches only the workloads that declared a route. Native plugins are host code: their own connections follow their configuration, not any workload's grants.

Every peer is untrusted, in both directions. The host parses HTTP, TLS, NATS and OCI registry traffic from peers, and no malformed request or response may crash it or corrupt its memory. Request, response and message bodies pass to the component unparsed; how it handles their content (XML, JSON or anything else) is its own concern, confined to its sandbox.

## Security goals

This section details goals that are expected to hold for a host with default settings.

A breach of one is a vulnerability unless mentioned in [Known limitations](#known-limitations).

1. **Sandbox.** A component runs only as WebAssembly in its own instance. Whatever bytes it contains, it cannot execute native code, read or write memory outside its own linear memories and tables, call a host function it was not linked to, or crash or corrupt the host.
2. **Grants.** A component reaches only the hosts, addresses, files, config, secrets and interfaces its spec grants.
3. **Isolation.** A workload cannot read or modify another workload's memory, files, connections or state unless they are explicitly shared, and reaches another workload's services only through interfaces linked between them.
4. **Host.** A workload cannot reach the host's own credentials, configuration, kernel filesystems or listeners. It reaches the machine's loopback only when the operator and its spec both allow it, and never a port the host owns that way.
5. **Egress.** Raw socket egress reaches only the addresses that the spec and the host's address policy permit, which by default excludes loopback, link-local (including cloud metadata) and multicast.
6. **Resources.** One workload cannot exhaust the host's memory, connections or file descriptors beyond the limits the operator set.
7. **Untrusted input.** No request, response, message or registry artifact from a peer can crash the host or corrupt its memory.

## Operator settings that weaken a goal

Certain settings may be set to `count` which enable host migration but weaken the security model. The `count` setting, where applicable, reports what policy would have refused, but allows the operation.

Private address ranges (RFC 1918, ULA, CGNAT) are reachable unless the operator sets `--deny-private-ranges`, since reaching a sibling service on a private address is the ordinary in-cluster case.

| Setting | Goal | Effect |
| --- | --- | --- |
| `--socket-egress=count` | 5 | Raw socket egress is counted, not enforced. |
| `--guest-memory-mode=count` | 6 | Total guest memory is counted, not enforced. |
| `--guest-memory-mode=growth` | 6 | Up to 2 MiB of each memory's first allocation (an instance's initial memory) is not charged to the budget. `--max-process-memory` refuses new instances and growth once the host is over its limit, but cannot stop admitted instances writing the initial memory they hold. The host is overcommitted by however much of it they come to touch, up to `--max-initial-guest-memory`: four times `--max-guest-memory` unless set, so by default about three times a container limit the budget was derived from. |
| `--memory-reservation` below 4 GiB, `--memory-guard-size` below 32 MiB | 6 | Guest memory isolation rests on wasmtime's explicit bounds checks alone, without the unmapped guard region behind each memory that otherwise catches an out-of-bounds access. Guest memory access is also slower. |
| `--deny-special-ranges=false` | 5 | Loopback, link-local and metadata addresses become reachable. |
| `--http-local-routing` | 3 | Any workload can claim a hostname and receive its neighbors' requests, in plaintext. |
| `--wasi-webgpu` | 1 | The GPU and its driver sit outside wasmtime's isolation. |
| `--allow-insecure-registries` | — | Pulls use plain HTTP, so registry credentials travel unencrypted. |

The following are enforced in every mode:

- `wasi:http` requests to a host outside `allowedHosts`.
- The machine's own loopback, which needs `--allow-host-loopback` *and* a grant in the workload's spec, and never reaches a port the host owns.
- Listening: a component cannot accept connections. A service listens on its workload's virtual network, and outside it only on ports its spec publishes.
- `hostPath` volumes: one must lie inside an `--allowed-host-path` directory, and none may expose a reserved host path, a kernel filesystem, or the `emptyDir` scratch holding other workloads' volumes.
- Per-workload and host-wide connection limits.

## What we treat as a vulnerability

Breaches of security goals that are not included in [Known limitations](#known-limitations), which should be covered by current controls are considered vulnerabilities.

For example:

- A component, request, response or registry artifact that crashes the host, hangs it, or corrupts its memory.
- A component reaching something its spec does not grant, or bypassing a control that is enforced in every mode.
- Any cross-workload effect beyond contention for shared resources within their configured limits: reading, writing, impersonating, or reaching another workload.
- Forging the workload identity that the host asserts to a plugin.
- A component or an unauthenticated peer making the host use resources far out of proportion to the request.
- A secret reaching a workload it is not bound to, a log, or a metric.

## What we do not treat as a vulnerability

- **Attacks that need a trusted actor.** This includes an operator flag, NATS credentials for the control plane, a native plugin, or a spec that grants the access in question.
- **Documented relaxations.** A count mode, or reachable private ranges, allowing what this document says it allows is by design. Bypassing an enforcing setting is a vulnerability.
- **Bugs confined to the sandbox.** A component that traps, misbehaves or computes the wrong answer harms only itself.
- **Resource use within limits.** A component may use everything it is entitled to, including the time it takes to compile a component the spec chose to run. A binary that crashes the compiler is a vulnerability; one that is merely slow to compile is not.
- **Side channels.** Spectre-class and timing side channels between workloads sharing a host. See [Known limitations](#known-limitations).
- **Defense-in-depth gaps.** Missing hardening that no attack demonstrates. We welcome these as ordinary issues.
- **Upstream bugs.** These belong to wasmtime and other dependencies. Report them upstream; tell us too if wasmCloud needs a release to pick up the fix.

## Known limitations

- **The `hostPath` allowlist is host-wide.** Every workload on a host may mount any directory the operator allowed. It is a guardrail, not a multi-tenant storage policy; where tenants author workloads, refuse their raw `hostPath` volumes before they reach a host.
- **Scratch volumes are not size-limited.** Each `emptyDir` is removed when its workload stops, but nothing caps its size. Put `--scratch-root` on a volume with a size limit.
- **A socket grant by name trusts that name's DNS.** An `allowedHosts` name entry permits the addresses the workload's own lookup returned, so whoever controls the name decides where it points, within the address policy.
- **`wasi:http` egress is checked by name, not by address (goal 2).** `allowedHosts` limits the names a request may use, but nothing checks the address a name resolves to. A wildcard entry therefore reaches any address a DNS name points at, including loopback and cloud metadata.
- **The host's listeners are reachable at the machine's own address.** A workload whose `allowedHosts` and the private-range policy permit the host's own address can reach the host's ingress, or another workload's published port, there (goal 4).
- **One isolation layer.** Workloads on one host are separated by wasmtime alone. For mutually hostile tenants, give each its own host group or node.
- **CPU is not shared fairly.** The host reclaims calls that their callers have abandoned, but it does not cap a workload's share of CPU. Pod CPU limits bound the host as a whole.
- **Instance slots are host-wide.** The pooling allocator's instance limit is shared by every workload on a host, not partitioned between them.
- **Native plugins resolve their own names.** For a native plugin, the host checks the endpoint the plugin declares, but cannot filter the addresses a hostname resolves to. Prefer IP or CIDR entries in a native plugin's `allowedHosts`.
- **Wasm host component plugins are shared.** A bug in one is a cross-tenant bug. A plugin that fails restarts, but calls in flight at the time are lost.
- **Guest output reaches host logs.** Treat log content written by a guest as untrusted.
- **Workload IDs come from the control plane.** The host relies on each being unique. Reusing an ID would let a new workload inherit the old one's pooled connections.

## Operator responsibilities

wasmCloud is one layer. Each layer below it must hold for the guarantees above to mean anything.

- **Node and OS.** Patch the kernel and node OS. Give mutually hostile tenants separate nodes, since wasmtime is the only layer between workloads on one host.
- **Container.** Keep the chart's pod hardening for hosts: non-root, a read-only root filesystem, all capabilities dropped, the `RuntimeDefault` seccomp profile, and no service account token. Set pod CPU and memory limits.
- **Kubernetes.** Restrict who may create `Workload` resources, and use admission policy to constrain `hostPath`, `allowedHosts` and similar grants. Apply `NetworkPolicy` to host pods.
- **Control plane.** Secure NATS with TLS and authentication, and restrict who may publish to `runtime.host.>`.
- **Supply chain.** Pin components by digest, and decide which registries a host may pull from.
- **wasmCloud host.** Use count modes only while migrating, and return to enforce once nothing would be refused. Name credential files the host can read with `--reserved-host-path`; the host already reserves the files its own flags and configuration name. Keep hosts current, since a wasmtime fix reaches you only through a release.

## See also

- [SECURITY.md](SECURITY.md): how to report a vulnerability, and how we disclose.
- [wasmtime security](https://docs.wasmtime.dev/security.html), and [what wasmtime considers a vulnerability](https://docs.wasmtime.dev/security-what-is-considered-a-security-vulnerability.html).
