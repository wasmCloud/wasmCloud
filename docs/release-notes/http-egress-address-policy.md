# HTTP egress address policy (breaking change)

Outbound `wasi:http` and gRPC now apply the host's socket address policy to the addresses they dial.
The default enforcing policy blocks ungranted loopback, link-local addresses (including cloud metadata), and other special ranges,
even when `allowedHosts` permits the hostname or address literal.
Requests to a prohibited address return `DestinationIpProhibited`.
This affects `wash host`, embedded hosts, and `wash dev`.

To reach a backend on the host's loopback, use `http://host.wasmcloud.internal:3000` or an explicit `127.0.0.1` or `::1` destination.
IPv4-mapped `::ffff:127.0.0.1` is also accepted.
Declare the original destination in `allowedHosts` and the TCP port in `allowedHostLoopbackPorts`.
For example, in `.wash/config.yaml`:

```yaml
workload:
  allowedHosts: ["http://host.wasmcloud.internal:3000"]
  allowedHostLoopbackPorts: [3000]
```

Start `wash host` with `--allow-host-loopback`, or set `networking.allowHostLoopback: true` on the chart's applicable `runtime.hostGroups` entry.
`wash dev` already enables the host side of this gate; the component still needs its port grant.
An absent grant, the wrong port or transport, or a port owned by the host is refused.
Explicit loopback literals require these grants even with `--deny-special-ranges=false` or `--socket-egress=count`.
Each component's grant is checked on every request, including reuse of a workload's pooled connection.
HTTP, HTTPS, pooled gRPC, and the gRPC fallback use this route.
The original authority is preserved for the HTTP Host header and TLS verification.
HTTPS certificates must cover the requested hostname or IP address; IP literals do not send a TLS SNI hostname.

The hostname is resolved inside the host without a DNS lookup.
Ordinary hostnames (including `localhost`) and other address literals continue through the address policy;
a DNS answer pointing at loopback does not acquire the explicit loopback route.
Raw `wasi:sockets` loopback continues to address the workload's virtual network;
use `host.wasmcloud.internal` to select host loopback through that API.
`--socket-egress=count` counts ordinary address-policy refusals without enforcing them,
but the explicit host-loopback route still requires both grants and refuses host-owned ports.
