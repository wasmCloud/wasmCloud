# Contributing to wash

Thank you for your interest in contributing to wash! This document provides guidelines and information for contributors.

## Getting Started

If you have feature suggestions, find bugs, or have questions, please submit an issue [here][0].
Pull requests are welcome! The [good first issue][1] label is a great way to find a starting point for new contributors.

## Development Environment

### Prerequisites

- **Rust** (latest stable version)
- **Git**
- **WebAssembly targets**: `wasm32-wasip2` and `wasm32-wasip1` (installed via `rustup target add wasm32-wasip2 wasm32-wasip1`) are both needed to build the `wash-runtime` wasm test fixtures.

### Building from Source

```bash
git clone https://github.com/wasmcloud/wasmCloud.git
cd wasmCloud
cargo build
```

### Running Tests

The `wash-runtime` integration tests and benchmarks load precompiled wasm
components from `crates/wash-runtime/tests/wasm/`. Build them once with the
`xtask` task runner before running those tests (re-run it whenever you change
a fixture under `crates/wash-runtime/tests/fixtures/`):

```bash
# Build the wasm test fixtures (writes crates/wash-runtime/tests/wasm/*.wasm)
cargo xtask build-fixtures
```

```bash
# Run all tests
cargo test

# Run specific test suites
cargo test --lib
cargo test --bin wash
```

## Project Structure

This repository is a workspace. The `wash` CLI lives in `crates/wash` and the core Wasm runtime it depends on lives in `crates/wash-runtime`:

```text
crates/
├── bench-tools/            # Benchmarking utilities
├── wash/                   # wash CLI crate
│   └── src/
│       ├── cli/            # CLI structs and command handling
│       │   ├── mod.rs
│       │   └── <subcommand>.rs
│       ├── lib.rs          # Module exports
│       ├── main.rs         # The wash binary entrypoint
│       ├── config.rs       # Configuration management
│       └── new.rs          # Project creation functionality
└── wash-runtime/           # Core Wasm runtime powering wash
    └── src/
        ├── engine/         # Wasmtime engine setup and execution context
        │   ├── ctx.rs      # Store context (WASI, linking state)
        │   ├── value.rs    # Runtime value types
        │   └── workload.rs # Workload execution logic
        ├── host/           # Host-side interface implementations
        │   ├── allowed_hosts.rs        # Network allow-list enforcement
        │   ├── http.rs / http_p3.rs    # HTTP outbound host (P2 and P3)
        │   └── sysinfo.rs              # System information host
        ├── plugin/         # WASI and wasmCloud capability plugins
        │   ├── wasi_blobstore/         # wasi:blobstore implementation
        │   ├── wasi_config/            # wasi:config implementation
        │   ├── wasi_keyvalue/          # wasi:keyvalue implementation
        │   ├── wasi_logging/           # wasi:logging implementation
        │   ├── wasi_otel/              # OpenTelemetry WASI bridge
        │   ├── wasi_webgpu/            # wasi:webgpu implementation
        │   ├── wasmcloud_messaging/    # wasmCloud messaging capability
        │   └── wasmcloud_postgres/     # wasmCloud Postgres capability
        ├── sockets/        # WASI sockets host implementations (TCP/UDP/network)
        ├── washlet/        # Washlet runner (embedded Wasm plugin host)
        ├── lib.rs          # Crate root and module exports
        ├── observability.rs # Tracing and metrics setup
        ├── oci.rs          # OCI registry image pulling
        ├── types.rs        # Shared runtime types
        └── wit.rs          # WIT interface bindings
```

## Code Style and Conventions

### Rust Style Guidelines

All Rust code in this project must follow these conventions:

#### Error Handling

- **Never use `unwrap()`, `expect()`, or `panic!()`** - Use proper error handling with `Result` and `Option`
- Use `anyhow::Result` for functions that can return errors
- Add context to errors using `.context()` method: `operation().context("failed to perform operation")?`
- Error messages and log contexts should start with lowercase and not end with periods

#### String Formatting

- Use string interpolation: `format!("{value}")` instead of `format!("{}", value)`

#### Output and Logging

- **Never use `println!` or `eprintln!` for output** - Use the `CommandOutput` struct for all command results
- Use `tracing` crate macros for all logging: `info!()`, `debug!()`, `warn!()`, `error!()`, `trace!()`
- Use the `#[instrument]` macro for any operations that take longer than 100ms
- Instrumented functions should have descriptive names with verbs (e.g., "Building component", "Fetching template")

#### Environment Variables

- Prefix all environment variables with `WASH_` to avoid conflicts and ensure clarity

#### CLI Design

- Use the `CommandOutput` struct for all command return values
- Commands should return structured data that can be formatted as text or JSON
- Follow clap derive patterns for argument parsing

### Logging and Tracing

This CLI is instrumented with the `tracing` crate:

- Use `#[instrument(level = "debug", skip_all, name = "operation_name")]` for long-running functions
- Log levels should be appropriate:
  - `error!()` - Unrecoverable errors
  - `warn!()` - Recoverable issues or deprecated usage
  - `info!()` - User-facing progress information
  - `debug!()` - Developer debugging information
  - `trace!()` - Detailed execution flow

Example:

```rust
use tracing::{info, instrument};

#[instrument(level = "debug", skip_all, name = "building_component")]
pub async fn build_component(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
    info!("Building WebAssembly component");
    // ...
    Ok(CommandOutput::ok("Component built successfully", None))
}
```

## Submitting Changes

### Pull Request Process

1. **Fork the repository** and create your feature branch from `main`
2. **Write tests** for any new functionality
3. **Ensure all tests pass** with `cargo test`
4. **Follow the code style guidelines** outlined above
5. **Update documentation** if you're changing behavior or adding features
6. **Write clear commit messages** that explain what and why, not just what

### Commit Message Format

```
<type>: <description>

[optional body]

[optional footer]
```

Types:

- `feat:` - New feature
- `fix:` - Bug fix
- `docs:` - Documentation changes
- `style:` - Code style changes (formatting, etc.)
- `refactor:` - Code refactoring
- `test:` - Adding or updating tests
- `chore:` - Maintenance tasks

### Code Review

All submissions require review. We use GitHub pull requests for this process.

### When does my change ship?

Releases ship every two weeks: each Tuesday at 16:00 UTC on the train's cycle, the next
`vX.Y.Z` is cut from `main` automatically. Anything merged before the train leaves ships in
that release. See [RELEASE_RUNBOOK.md](./RELEASE_RUNBOOK.md#release-cadence) for details.

## Testing

- Write unit tests for new functionality
- Include integration tests for CLI commands when appropriate
- Test error cases and edge conditions
- Ensure tests are deterministic and don't depend on external services

## Documentation

- Update README.md if you're changing user-facing functionality
- Add inline documentation for public APIs
- Update command help text if you're modifying CLI behavior

## References

- [Rust Style Guide](https://doc.rust-lang.org/style-guide/) - Official Rust coding standards
- [WebAssembly Component Model](https://component-model.bytecodealliance.org/) - Learn about the component model
- [WASI Preview 2](https://github.com/WebAssembly/WASI/tree/main/preview2) - WebAssembly System Interface

[0]: https://github.com/wasmcloud/wasmCloud/issues/new/choose
[1]: https://github.com/wasmcloud/wasmCloud/issues?q=is%3Aopen+is%3Aissue+label%3A%22good+first+issue%22
