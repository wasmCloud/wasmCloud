# HTTP egress address policy (breaking change)

Outbound `wasi:http` and gRPC now apply the host's socket address policy to the addresses they dial.
The default enforcing policy blocks link-local addresses (including cloud metadata) and other special ranges,
even when `allowedHosts` permits the hostname or address literal.
Host loopback needs the operator's permission and an explicit component grant, as described below.
Requests to a prohibited address return `DestinationIpProhibited`,
and the host logs a warning; for a loopback address, the warning names the fix below.
Missing loopback permission or a host-owned loopback port returns `HttpRequestDenied` before dispatch.
This affects `wash host`, embedded hosts, and `wash dev`.

## Reaching a backend on the host's loopback

For HTTP-only access, keep a URL such as `http://localhost:3000` and explicitly grant that exact URL in `allowedHosts`:

```yaml
workload:
  allowedHosts: ["http://localhost:3000"]
  allowedHostLoopbackPorts: []
```

The host must enable `--allow-host-loopback` (or the chart's `networking.allowHostLoopback`).
`wash dev` enables that operator permission, but still requires the explicit component URL grant.
The grant fixes the host, HTTP scheme, and effective port: an omitted port means 80 for HTTP or 443 for HTTPS.
It also works for exact loopback literals such as `http://127.0.0.1:3000` and `http://[::1]:3000`.
`*`, suffix wildcards, and bare authorities such as `localhost:3000` do not grant host loopback.
The host resolves `localhost` internally to `::1` and `127.0.0.1`; a different hostname resolving to loopback stays subject to the normal address policy.
Host-owned ports remain refused, and each component's grant is checked before pooled connection reuse.
These rules apply to HTTP, HTTPS, and gRPC carried through `wasi:http`, including the gRPC fallback.

This grant does not permit raw sockets to reach the host.
Raw socket loopback remains the workload's virtual network, and raw access through `host.wasmcloud.internal` still needs `allowedHostLoopbackPorts`.

For a shared HTTP and raw socket grant, replace `http://localhost:3000` with `http://host.wasmcloud.internal:3000`,
and grant the component the TCP port in `allowedHostLoopbackPorts`.
HTTP and gRPC additionally need an `allowedHosts` entry permitting the request's scheme, host, and effective port.
The TCP port grant never overrides this HTTP policy, and an `allowedHosts` entry (even `*`) never replaces the port grant.
For example, in `.wash/config.yaml`:

```yaml
workload:
  allowedHosts: ["https://host.wasmcloud.internal:3000"]
  allowedHostLoopbackPorts: [3000]
```

This example permits HTTPS to port 3000 and refuses plain HTTP, even though both use TCP.
The HTTP allowlist is checked before every request, including pooled reuse.
Custom HTTP handlers receive a single exact URL grant for the authorized request, rather than an unrestricted allowlist.

The operator's half is `wash host --allow-host-loopback`, or `networking.allowHostLoopback: true` on the chart's applicable `runtime.hostGroups` entry.
`wash dev` already enables the operator's half; the component still needs its port grant.
A request needs both halves. An absent grant, the wrong port or transport, or a port owned by the host is refused.
Each component's grant is checked on every request, including reuse of a workload's pooled connection.
HTTP, HTTPS, pooled gRPC, and the gRPC fallback use this route.
HTTPS keeps `host.wasmcloud.internal` for certificate verification and TLS SNI, so the backend's certificate must cover that name.

The hostname is resolved inside the host without a DNS lookup.
Ordinary hostnames and non-loopback address literals continue through the address policy;
a loopback grant does not permit them to reach loopback or metadata addresses.
`--socket-egress=count` counts ordinary address-policy refusals without enforcing them,
but both explicit loopback routes still require operator and component permission and refuse host-owned ports.

Raw `wasi:sockets` loopback is unaffected: it remains the workload's virtual network.
