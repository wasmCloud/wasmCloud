# wash-runtime

[![Apache 2.0 License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](../../LICENSE)

**wash-runtime** is an opinionated Wasmtime wrapper that provides a runtime and workload API for executing WebAssembly components. It offers a simplified interface for embedding Wasm component execution in Rust applications with built-in support for WASI interfaces.

## Features

- **Component Model Runtime**: Native support for WebAssembly Component Model using Wasmtime
- **WASI Interface Support**: Built-in plugins for WASI HTTP, Config, Logging, Blobstore, and Key-Value
- **Workload API**: High-level API for managing and executing component workloads
- **Plugin System**: Extensible architecture for custom capability providers
- **OCI Integration**: Optional support for pulling components from OCI registries
- **Hot-Reload Ready**: Designed for development workflows with fast iteration

## Usage

### Basic Example

```rust
use std::sync::Arc;
use std::collections::HashMap;

use wash_runtime::{
    engine::Engine,
    host::{HostBuilder, HostApi,
      http::{Ingress, DynamicRouter},
  },
    plugin::{
        wasi_config::DynamicConfig,
    },
    types::{WorkloadStartRequest, Workload},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a Wasmtime engine
    let engine = Engine::builder().build()?;

    // Configure plugins
    let http_router = DynamicRouter::default();
    let ingress = Ingress::new(http_router, "127.0.0.1:8080".parse()?).await?;
    let wasi_config_plugin = DynamicConfig::default();

    // Build and start the host
    let host = HostBuilder::new()
        .with_engine(engine)
        // if a handler is not provided, a 'deny all' implementation
        // will be used for outgoing http requests
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(wasi_config_plugin))?
        .build()?;

    let host = host.start().await?;

    // Start a workload
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "test-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;

    Ok(())
}
```

### Cargo Features

The crate supports the following cargo features:

- `oci`: OCI registry integration for pulling components
- `wasi-blobstore` (default): Blob storage interface
- `wasi-config` (default): Runtime configuration interface
- `wasi-http` (default): HTTP client and server support via `wasmtime-wasi-http`
- `wasi-keyvalue` (default): Key-value storage interface
- `wasi-logging` (default): Logging interface
- `wasi-otel` (default): OpenTelemetry interface
- `wasi-webgpu` WebGPU interface

### Architecture

wash-runtime provides three main abstractions:

1. **Engine**: Wasmtime configuration and component compilation
2. **Host**: Runtime environment with plugin management
3. **Workload**: High-level API for managing component lifecycles

## Testing

The integration tests and benchmarks load precompiled wasm components from
`tests/wasm/`. Build them once before running them:

```bash
# from the repo root
cargo xtask build-fixtures
```

Re-run it after editing a fixture under [`tests/fixtures/`](./tests/fixtures/)
(see that directory's README for details).

## Plugin TLS grants

A plugin's `allowedHosts` entries can select the TLS a connection to that host
uses: which CAs to verify the server with, and which client identity to
present. The material is declared once, in named catalogs, and a grant names
what it uses, so the egress policy never carries key material:

```yaml
trustBundles:
  nats-ca:
    ca: tls/ca.crt
    roots: replace       # add (the default) keeps the public roots too
identities:
  nats-client:
    cert: tls/client.crt
    key: tls/client.key
    refresh: 30s         # optional: re-read on this interval
host:
  plugins:
    - id: wasmcloud-nats
      allowedHosts:
        - host: "tls://nats.internal:4222"
          tls: { trust: nats-ca, identity: nats-client }
      allowedIpNameLookups: ["nats.internal"]
```

`tls: {}` uses the platform's default roots and presents no identity. Relative
paths resolve against the project directory, and every file is read when the
host starts, so a missing or malformed one fails startup rather than the first
connection. An expired client certificate is never presented; with `refresh`,
a rewritten one reaches new connections without a restart, and a failed read
keeps the running credential.

A `tls` block covers exactly what its entry grants: the host, and the port and
scheme the entry pins. An identity on `tls://shared.internal:4222` is not
presented to `https://shared.internal:8443`, and two services behind one name
can select different identities. A scheme-pinned entry without a port grants
that scheme's default port, so `https://api.internal` covers 443 and nothing
else. An entry pinned to another protocol's scheme never applies: an
`https://` entry does not reach a NATS connection on the same port, where
`nats://` and `tls://` are the two spellings of one protocol. Where several
entries match, the most specific wins; two entries with the same host, port
and scheme and different `tls` blocks are refused, as are two aliases of one
protocol (`nats://` and `tls://`) that disagree on the same endpoint. A TLS client that knows only the server name, such as one
wrapping a socket the plugin opened, is answered only by an entry that pins no
port and no scheme, so granting a whole host is always written as such.

`*`, and a one-label suffix such as `*.com`, may only use `tls: {}`, since a
trust bundle or identity there would apply to every destination, or every host
under a top-level domain; name the host, or a `*.suffix` of at least two
labels, the material is for.

An identity on a plugin's grant is the plugin's own service identity. The
plugin authenticates as it for every workload it serves, and the host does not
check which workload asked, so grant one only to a plugin trusted to authorize
its callers' operations itself. Presenting a calling workload's own identity is
a separate, delegated form that is not built yet.

A plugin that cannot apply a `tls` block fails to load rather than connecting
without it. `wasmcloud-nats` dials the granted servers with that trust and
requires TLS; another plugin applies it by implementing
`HostPlugin::configure_tls_policy`.

`wash host` keeps workload `hostPath` volumes away from every file the catalogs
name, as it does for its other credentials, so a workload cannot read a
plugin's key.

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](../../LICENSE) file for details.
