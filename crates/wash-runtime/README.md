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

### Embedded commands

An embedder can resolve a workload and run one of its components as a command:

```rust,ignore
use wash_runtime::engine::command::CommandOptions;

let resolved = engine.initialize_workload("shell-run", workload)?
    .resolve(Some(&plugins), &bindings, &host_ref, &meters).await?;
let options = CommandOptions::new()
    .args(["agent", "hello"])
    .stdin(wasmtime_wasi::cli::stdin());
let result = resolved.run_command(&component_id, options).await;
let cleanup = resolved.unbind_all_plugins().await;
let exit_code = result?.exit_code;
cleanup?;
```

`run_command` runs a WASIp2 or WASIp3 `wasi:cli/run` exactly once in a fresh
store, using the resolved workload's grants, plugins, environment and volumes.
Arguments include the program name.
Stdin defaults to EOF; stdout and stderr default to the host's streams.
All three accept custom WASI streams, including memory pipes and terminal adapters.
Successful and unsuccessful command returns become exit codes 0 and 1;
`wasi:cli/exit` preserves its exit code, and traps return errors.
Dropping the invocation future drops its store.
Services retain their existing supervision behavior.

### Structured streams across component-plugin stores

Scalar and string stream/future payloads work without configuration.
For application-defined records, variants, lists, options and results,
register the generated Wasmtime binding type before loading plugins or resolving
workloads:

```rust,ignore
// `payload_type` is the Type from a compiled component's StreamType::ty()
// or FutureType::ty(), e.g. the record used by stream<entry>.
engine.stream_payloads().register::<bindings::Entry>(payload_type)?;
```

Registrations belong to the engine and propagate to both workload and plugin
stores, including restarted plugin stores.
The bridge uses the same bounded, backpressured pumps as scalar streams;
it does not collect a stream before returning it.
Wasmtime checks the registered Rust type against each source handle before
transferring it.
Payloads containing resources, nested streams/futures or error contexts are
rejected because their handles belong to the source store.
Native Rust plugins can use their generated typed bindings directly and need
no cross-store registration.

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

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](../../LICENSE) file for details.
