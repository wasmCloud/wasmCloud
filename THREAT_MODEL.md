# wasmCloud Threat Model

This document states what wasmCloud defends against, whom it trusts, and what we treat as a security vulnerability. It covers this repository. To report a vulnerability, follow [SECURITY.md](SECURITY.md).

## Scope

In scope:

- The host: `wash host` and the `wash-runtime` crate, including the plugins it ships.
- `runtime-operator`, `runtime-gateway`, and the Helm charts in `charts/`.
- Release builds with default features, on the platforms we publish.

Out of scope:

- `wash dev` and other developer workflows. They run code the developer chose, on the developer's own machine.
- Features marked experimental, and non-default build features.
- Applications deployed onto wasmCloud. Report those to their maintainers.

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

1. **Component ↔ host.** This is the wasmtime sandbox, plus the interfaces the host links into it. A component holds no ambient authority; it has only the imports its spec grants.
2. **Workload ↔ workload.** Each workload has its own stores, memory and virtual network: `127.0.0.1` inside a workload reaches only that workload. Workloads interact only through interfaces the host links between them.
3. **Workload ↔ plugin.** The host asserts the caller's workload identity to the plugin, and a guest cannot set or forge it. A plugin must authorize on that identity, never on an identity passed in a call's arguments.
4. **Host ↔ network.** Egress is limited by the workload's `allowedHosts` and by the host's address policy. Ingress reaches only the workloads that declared a route.

## Security goals

When every control below is set to enforce, the host guarantees that:

- A component cannot escape the sandbox or corrupt host memory.
- A component reaches only the hosts, addresses, paths, config and secrets its spec grants.
- A workload cannot read or modify another workload's memory, files, connections or state, and cannot reach another workload's services except through interfaces linked between them.
- A workload reaches the machine's own loopback only when the operator and its spec both allow it, and never a port the host owns (its ingress, its control plane, or another workload's published port) that way.
- Raw socket egress reaches only the addresses that the spec and the host's address policy permit, which by default excludes loopback, link-local (including cloud metadata) and multicast.
- One workload cannot exhaust the host's connections, file descriptors or guest memory beyond the limits the operator set.

## Defaults are not the enforced posture

Several controls shipped after workloads already depended on the behavior they restrict. Those controls **count** violations by default rather than refusing them, so that upgrading a host breaks no running workload. Watch what they report, then switch to enforce:

| Control | Default | To enforce | Reported as |
| --- | --- | --- | --- |
| Raw socket egress | count | `--socket-egress=enforce` | a debug log line per refusal |
| Total guest memory | count | `--guest-memory-mode=enforce` | the `guest_memory.would_refuse` metric |
| Private address ranges (RFC 1918, ULA, CGNAT) | allowed | `--deny-private-ranges` | — |

The following are enforced in every mode:

- `wasi:http` requests to a host outside `allowedHosts`.
- The machine's own loopback, which needs `--allow-host-loopback` *and* a grant in the workload's spec, and never reaches a port the host owns.
- Listening: a component cannot accept connections. A service listens on its workload's virtual network, and outside it only on ports its spec publishes.
- Per-workload and host-wide connection limits.

## What we treat as a vulnerability

- Escaping the sandbox, or corrupting host memory, from guest input.
- A component reaching something its spec does not grant while the relevant control enforces, or bypassing a control that is enforced in every mode.
- Any cross-workload effect beyond contention for shared resources within their configured limits: reading, writing, impersonating, or reaching another workload.
- Forging the workload identity that the host asserts to a plugin.
- A component or an unauthenticated network peer that crashes the host, hangs it, or makes it use resources far out of proportion to the request.
- A secret reaching a workload it is not bound to, a log, or a metric.

## What we do not treat as a vulnerability

- **Attacks that need a trusted actor.** This includes an operator flag, NATS credentials for the control plane, a native plugin, or a spec that grants the access in question.
- **Documented permissive defaults.** Count mode allowing a connection is by design. Bypassing the enforcing setting is a vulnerability.
- **Bugs confined to the sandbox.** A component that traps, misbehaves or computes the wrong answer harms only itself.
- **Resource use within limits.** A component may use everything it is entitled to, including the time it takes to compile a component the spec chose to run.
- **Side channels.** Spectre-class and timing side channels between workloads sharing a host. See [Known limitations](#known-limitations).
- **Defense-in-depth gaps.** Missing hardening that no attack demonstrates. We welcome these as ordinary issues.
- **Upstream bugs.** These belong to wasmtime and other dependencies. Report them upstream; tell us too if wasmCloud needs a release to pick up the fix.

## Known limitations

- **`hostPath` volumes are not checked.** The host mounts whatever path a spec names, so only operators should author them.
- **Scratch volumes are not reclaimed or size-limited.** An `emptyDir` volume's directory stays on disk after its workload stops.
- **`wasi:http` egress is checked by name, not by address.** `allowedHosts` limits the names a request may use, but nothing checks the address a name resolves to. A wildcard entry therefore reaches any address a DNS name points at, including loopback and cloud metadata.
- **Published ports are reachable at the machine's own address.** A workload whose `allowedHosts` and the private-range policy permit the host's own address can reach another workload's published port there.
- **One isolation layer.** Workloads on one host are separated by wasmtime alone. For mutually hostile tenants, give each its own host group or node.
- **CPU is not shared fairly.** The host reclaims calls that their callers have abandoned, but it does not cap a workload's share of CPU. Pod CPU limits bound the host as a whole.
- **Instance slots are host-wide.** The pooling allocator's instance limit is shared by every workload on a host, not partitioned between them.
- **Native plugins resolve their own names.** For a native plugin, the host checks the endpoint the plugin declares, but cannot filter the addresses a hostname resolves to. Prefer IP or CIDR entries in a native plugin's `allowedHosts`.
- **Wasm host component plugins are shared.** A bug in one is a cross-tenant bug. A plugin that fails restarts, but calls in flight at the time are lost.
- **Guest output reaches host logs.** Treat log content written by a guest as untrusted.
- **Workload IDs come from the control plane.** The host relies on each being unique. Reusing an ID would let a new workload inherit the old one's pooled connections.

## Operator responsibilities

- Secure NATS with TLS and authentication, and restrict who may publish to `runtime.host.>`.
- Restrict who may create `Workload` resources, and use admission policy to constrain `hostPath`, `allowedHosts` and similar grants.
- Switch count-mode controls to enforce once their counters are quiet.
- Keep credentials the host can read out of any path a `hostPath` volume could name.
- Apply Kubernetes `NetworkPolicy`, and pod CPU and memory limits.
- Pin components by digest, and decide which registries a host may pull from.
- Keep hosts current, since a wasmtime fix reaches you only through a release.

## Related

- [SECURITY.md](SECURITY.md): how to report a vulnerability, and how we disclose.
- [wasmtime security](https://docs.wasmtime.dev/security.html), and [what wasmtime considers a vulnerability](https://docs.wasmtime.dev/security-what-is-considered-a-security-vulnerability.html).
