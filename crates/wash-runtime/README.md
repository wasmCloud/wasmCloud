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

A plugin's `allowedHosts` entries can supply CA roots and a client certificate
for HTTPS, gRPC over HTTPS, and the host's TLS interfaces:

```yaml
host:
  plugins:
    - id: database
      file: database.wasm
      allowedHosts:
        - host: "db.internal:443"
          tls:
            ca: tls/ca.crt
            roots: replace
            clientCert: tls/client.crt
            clientKey: tls/client.key
            required: true
      allowedIpNameLookups: ["db.internal"]
```

Files are loaded at startup. HTTPS requests from both WASI HTTP versions use
the matching grant automatically, including the gRPC transport. Connections
and TLS session caches are isolated by plugin and loaded trust configuration.
Custom HTTP handlers must support the grant or refuse the request.

Without `required: true`, the TLS block configures host-performed TLS. A
component with raw socket permission can still send plaintext or implement
its own TLS. Importing a TLS interface does not prove the component uses it.

If **any** grant sets `required: true`, the component plugin cannot create or
use raw TCP or UDP sockets, even through wildcard or loopback grants. It also
cannot declare listening ports. Its outgoing HTTP requests must use HTTPS and
match a TLS grant; plaintext requests and HTTPS without declared trust fail.
These restrictions hold even when the host's socket policy is in count mode.

For other protocols, import `wasmcloud:tls/dialer@0.1.0` and call
`connect("tls://db.internal:443")`. The host checks the endpoint grant, DNS
permission, resolved addresses, loopback grants, and connection quota, then
completes TLS before returning a connection. The connection's `send` and
`receive` streams carry application bytes. The host never gives the component
a raw socket. This API requires TLS from the start; it does not implement
STARTTLS.

`wasmcloud:tls/client` and `wasi:tls/client` remain stream transforms for
components that have raw transport access. The restriction covers this
plugin's host networking interfaces. Other capabilities explicitly granted to
the plugin have their own policies; it does not impose TLS on another
component's or native plugin's connections.

### Upgrading a plugin that already imports `wasi:tls`

Before plugin TLS grants, a component plugin's `wasi:tls` import (in a build
with the `wasi-tls` feature) trusted the host's default roots. It now takes
its trust from the grant, and a handshake to a host no grant declares `tls`
for is refused. The plugin still loads, with a warning, so the failure shows
up at the first connection. To keep trusting the public roots, add an empty
`tls` block to each entry the plugin handshakes with:

```yaml
allowedHosts:
  - host: "api.example.com:443"
    tls: {}
```

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](../../LICENSE) file for details.
