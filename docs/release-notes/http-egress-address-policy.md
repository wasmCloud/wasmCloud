# HTTP egress address policy (breaking change)

Outbound `wasi:http` and gRPC now apply the host's socket address policy to the addresses they dial.
The default enforcing policy blocks loopback, link-local addresses (including cloud metadata), and other special ranges,
even when `allowedHosts` permits the hostname or address literal.
Requests to a prohibited address return `DestinationIpProhibited`,
and the host logs a warning; for a loopback address, the warning names the fix below.
This affects `wash host`, embedded hosts, and `wash dev`.

## Reaching a backend on the host's loopback

Replace `http://localhost:3000` with `http://host.wasmcloud.internal:3000`,
and grant the component the TCP port in `allowedHostLoopbackPorts`.
That port grant is the component's whole grant for `host.wasmcloud.internal`, over HTTP, gRPC, and raw sockets alike:
no `allowedHosts` entry is needed for it, and an `allowedHosts` entry (even `*`) does not stand in for it.
For example, in `.wash/config.yaml`:

```yaml
workload:
  allowedHostLoopbackPorts: [3000]
```

The operator's half is `wash host --allow-host-loopback`, or `networking.allowHostLoopback: true` on the chart's applicable `runtime.hostGroups` entry.
`wash dev` already enables the operator's half; the component still needs its port grant.
A request needs both halves. An absent grant, the wrong port or transport, or a port owned by the host is refused.
Each component's grant is checked on every request, including reuse of a workload's pooled connection.
HTTP, HTTPS, pooled gRPC, and the gRPC fallback use this route.
HTTPS keeps `host.wasmcloud.internal` for certificate verification and TLS SNI, so the backend's certificate must cover that name.

The hostname is resolved inside the host without a DNS lookup.
Ordinary hostnames and address literals continue through the address policy;
a loopback grant does not permit them to reach loopback or metadata addresses.
`--socket-egress=count` counts ordinary address-policy refusals without enforcing them,
but the explicit host-loopback route still requires both grants and refuses host-owned ports.

Raw `wasi:sockets` loopback is unaffected: it remains the workload's virtual network.
