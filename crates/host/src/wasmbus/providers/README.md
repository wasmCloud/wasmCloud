# wasmCloud Internal Capability Providers

[Capability providers](https://wasmcloud.com/docs/concepts/providers/) are [wasmCloud host](https://wasmcloud.com/docs/concepts/hosts/) extensions, which make external services available to WASI `Components` running on the host. While in accordnce with security and operational best practices the capability providers are mostly implemented as external executables, some of the broadly used WASI and wasmCloud interfaces are implemented as internal, or built-in, host extensions.

These could be further divded into two categories:

- Core capabilities such as access to `logging`, `configuration`, and `clocks`, which are built into the fabric of the wasmCloud platform, and are always `enabled` and available.
- Frequently used capabilities, including `http-client`, `http-server`, and `messaging-nats`, which are implemented as internal host extensions, have alternative external providers, and are `disabled` by default.

These optional built-in providers offer the following capabilities:

| Capability | Interface | Features |
|------------|-----------|----------|
| `http-client-provider` | `wasi-http/outgoing-handler` | `path mode` and `address mode` |
| `http-server-provider` | `wasi-http/incoming-handler` | `path mode` and `address mode` |
| `messaging-nats-provider` | `wasmcloud:provider-messaging-nats` | `pub/sub` and `request/response` |

## Enabling Internal Providers

To enable these providers, add the following to the host configuration:

```bash
# NOTE: Only include the providers you need
WASMCLOUD_EXPERIMENTAL_FEATURES="builtin-http-server,builtin-http-client,builtin-messaging-nats" wash up --experimental --detached
```

## Application Manifest Configuration

When specifying the capability provider's image URI, in [Application manifests](https://wasmcloud.com/docs/concepts/applications/#application-manifests), for an internal provider, use the following format:

```yaml
- name: sample-internal-provider
  type: capability
  properties:
    image: wasmcloud+builtin://http-client # or wasmcloud+builtin://http-server, wasmcloud+builtin://messaging-nats
```

## Internal Providers Configuration

The internal capability providers are configured exactly like their external counterparts. For configuration option details, please refer to their corresponding documentation under [wasmCloud crate](https://github.com/wasmCloud/wasmCloud/tree/main/crates) directory.
