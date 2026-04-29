# wasmCloud Rust Templates

Project templates for building wasmCloud components and services with Rust.

## Available Templates

| Template | Description |
|---|---|
| [http-hello-world](./http-hello-world/) | Minimal HTTP handler component using `wstd` |
| [http-client](./http-client/) | Component that makes outgoing HTTP requests |
| [http-handler](./http-handler/) | HTTP handler component with routing via `wstd-axum` |
| [http-kv-handler](./http-kv-handler/) | HTTP handler component backed by `wasi:keyvalue` |
| [http-api-with-distributed-workloads](./http-api-with-distributed-workloads/) | HTTP API that delegates processing to background workers via messaging |
| [service-tcp](./service-tcp/) | Service-and-component template demonstrating the wasmCloud service model with `wasi:sockets` and `tokio` |

## Template Structure

Each template follows a similar structure:

```
template-name/
├── .wash/
│   └── config.yaml      # wash CLI configuration
├── src/
│   └── *.rs             # Rust source code
├── wit/
│   └── world.wit        # Component world definition
└── Cargo.toml           # Rust package configuration
```

### Template usage

Each template may be used with `wash new`. For example, to create a new project with the `service-tcp` template:

```bash
wash new https://github.com/wasmCloud/wasmCloud.git --name my-service --subfolder templates/service-tcp
```

### Template conventions

Every template follows the convention of namespace as `wasmcloud`, package as `templates`, and the world is prefixed with the language. We version our templates for easy future updates (e.g. when adding support for WASIP3).

```wit
package wasmcloud:templates@0.1.0;
```
