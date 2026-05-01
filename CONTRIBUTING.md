# Contributing to wash

Thank you for your interest in contributing to wash! This document provides guidelines and information for contributors.

## Getting Started

If you have feature suggestions, find bugs, or have questions, please submit an issue [here][0].
Pull requests are welcome! The [good first issue][1] label is a great way to find a starting point for new contributors.

## Development Environment

### Prerequisites

- **Rust** (latest stable version)
- **Git**
- **WebAssembly targets**: `wasm32-wasip2` (installed via `rustup target add wasm32-wasip2`)

### Building from Source

```bash
git clone https://github.com/wasmcloud/wasmCloud.git
cd wasmCloud
cargo build
```

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test suites
cargo test --lib
cargo test --bin wash
```

## Project Structure

The `wash` crate is organized as a single crate with both binary and library targets:

```text
src/                # The wash binary
└── main.rs
crates/wash/src/
├── cli/            # CLI structs and command handling
│   ├── mod.rs
│   └── <subcommand>.rs
├── <subcommand>.rs # Reusable types and libraries for commands
├── lib.rs          # Module exports
├── config.rs       # Configuration management
└── new.rs          # Project creation functionality
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
