# HTTP egress address policy (breaking change)

Outbound `wasi:http` and gRPC now apply the host's socket address policy to the addresses they dial.
The default enforcing policy blocks loopback, link-local addresses (including cloud metadata), and other special ranges,
even when `allowedHosts` permits the hostname or address literal.
Requests to a prohibited address return `DestinationIpProhibited`.
This affects `wash host`, embedded hosts, and `wash dev`.

To reach a backend on the host's loopback, replace `http://localhost:3000` with `http://host.wasmcloud.internal:3000`.
Declare the hostname in `allowedHosts` and the TCP port in `allowedHostLoopbackPorts`.
For example, in `.wash/config.yaml`:

```yaml
workload:
  allowedHosts: ["http://host.wasmcloud.internal:3000"]
  allowedHostLoopbackPorts: [3000]
```

Start `wash host` with `--allow-host-loopback`, or set `networking.allowHostLoopback: true` on the chart's applicable `runtime.hostGroups` entry.
`wash dev` already enables the host side of this gate; the component still needs its port grant.
An absent grant, the wrong port or transport, or a port owned by the host is refused.
Each component's grant is checked on every request, including reuse of a workload's pooled connection.
HTTP, HTTPS, pooled gRPC, and the gRPC fallback use this route.
HTTPS keeps `host.wasmcloud.internal` for certificate verification and TLS SNI, so the backend's certificate must cover that name.

The hostname is resolved inside the host without a DNS lookup.
Ordinary hostnames and address literals continue through the address policy;
a loopback grant does not permit them to reach loopback or metadata addresses.
`--socket-egress=count` counts ordinary address-policy refusals without enforcing them,
but the explicit host-loopback route still requires both grants and refuses host-owned ports.
