# 📡 WADM Status Provider

This capability provider implements the [`wasmcloud:wadm`][wasmcloud-wadm-wit] WIT package, which enables components to interact with applications receive status updates from WADM from lattices.

This provider handles concurrent component connections and status subscriptions. Components linked to it should specify configuration at link time (see [the named configuration settings section](#named-configuration-settings) for more details).

[wasmcloud-wadm-wit]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-wadm/wit

## 👟 Quickstart

To get this provider started quickly, you can start with:

```console
wash start provider ghcr.io/wasmcloud/wadm-provider:0.1.0
```

The easiest way to start a WADM provider with configuration specified, and a component that uses it is with [wasmCloud Application Deployment Manager][wadm].

<details>
<summary>Example manifest for a status receiver component</summary>

```yaml
apiVersion: core.oam.dev/v1beta1
kind: Application
metadata:
  name: wadm-status-example
  annotations:
    version: v0.0.1
    description: WADM Status Receiver example
spec:
  components:
    # A capability provider that enables WADM status updates
    - name: wadm-client
      type: capability
      properties:
        image: ghcr.io/wasmcloud/wadm-provider:0.1.0
        config:
          - name: default-wadm
            properties:
              ctl_host: "127.0.0.1"
              ctl_port: "4222"
              lattice: "default"

    # A component that receives status updates
    - name: status-receiver
      type: component
      properties:
        image: file://./build/wadm_status_receiver_s.wasm
      traits:
        - type: spreadscaler
          properties:
            instances: 1
        - type: link
          properties:
            target:
              name: wadm-client
              config:
                - name: default-wadm-sub
                  properties:
                    app_name: "my-app"  # Application to monitor
            namespace: wasmcloud
            package: wadm
            interfaces: [handler]
```

</details>

[wadm]: https://github.com/wasmCloud/wadm

## 📑 Named Configuration Settings

Configuration can be specified via named configuration to the provider and referenced via link definitions.

New named configuration can be specified by using `wash config put`.

| Property | Default | Description |
|----------|---------|-------------|
| `ctl_host` | `0.0.0.0` | NATS control interface host |
| `ctl_port` | `4222` | NATS control interface port |
| `ctl_jwt` | - | JWT for NATS authentication |
| `lattice` | `default` | Lattice name |
| `app_name` | - | Application name to receive updates for (required for subscriptions) |
| `js_domain` | - | Optional JetStream domain |
| `api_prefix` | - | Optional API prefix |

## 🔐 Secret Settings

Sensitive values should be specified via *secrets*.

New secrets can be specified by using `wash secrets put`.

| Property | Description |
|----------|-------------|
| `ctl_seed` | Seed for NATS authentication |

## Authentication Options

The provider supports multiple authentication methods:

1. JWT and Seed combination
2. Credentials file
3. TLS authentication

These can be configured through the following settings:

| Property | Description |
|----------|-------------|
| `ctl_credsfile` | Path to credentials file |
| `ctl_tls_ca_file` | TLS CA certificate file |
| `ctl_tls_first` | Whether to perform TLS handshake first |

## 📦 Building a PAR

To build a [Provider Archive (`.par`/`.par.gz`)][par] for this provider, first build the project with `wash`:

```console
wash build
```

Then run `wash par`:

```console
wash par create \
  --compress \
  --binary target/debug/wadm-provider \
  --vendor wasmcloud \
  --version 0.1.0 \
  --name wadm-provider
```

[par]: https://wasmcloud.com/docs/developer/providers/build
