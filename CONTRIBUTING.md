# Contributing to wash

Thank you for your interest in contributing to wash! This document provides guidelines and information for contributors.

## Getting Started

If you have feature suggestions, find bugs, or have questions, please submit an issue [here][0].
Pull requests are welcome! The [good first issue][1] label is a great way to find a starting point for new contributors.

## Reporting Bugs

Report bugs as [GitHub issues][2] using the **Bug report** template. Search the [open issues][3] first, and add to an existing report if one matches.

A useful report includes:

- What you ran and what happened, with the full error message or logs.
- What you expected to happen instead.
- The smallest set of steps or commands that reproduces it. If it needs files, link a public
  repository with a minimal reproduction instead of attaching zip files or other archives.
- Your versions: the output of `wash --version`, and for Kubernetes deployments the runtime-operator chart version.
- Your operating system and architecture, and your Kubernetes version if one is involved.

Don't report security vulnerabilities as issues. Follow [SECURITY.md](SECURITY.md) to report them privately.

## Development Environment

### Prerequisites

- **Rust** (latest stable version)
- **Git**
- **WebAssembly targets**: `wasm32-wasip2` and `wasm32-wasip1` (installed via `rustup target add wasm32-wasip2 wasm32-wasip1`) are both needed to build the `wash-runtime` wasm test fixtures.
- **wasm-component-ld** (0.5.23 or newer): needed to link the wasm test fixtures — see the [warning in the README](README.md#building-from-source) for the install and linker setup.

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

### Rust coverage

Install the pinned coverage tool and the coverage-only nightly toolchain:

```bash
source .github/tool-versions.env
cargo install cargo-llvm-cov --locked --version "$CARGO_LLVM_COV_VERSION"
cargo xtask coverage setup
cargo xtask coverage
```

`cargo xtask coverage` builds the Wasm fixtures without coverage instrumentation,
then runs the native Rust workspace tests with the same optional features as
`wash.yml`, including the ignored Docker/registry tests. Docker must be running;
registry access and the usual fixture build prerequisites are required. Coverage
builds use a separate Cargo target directory, managed by cargo-llvm-cov. Only
coverage uses the dated nightly pinned in `xtask/coverage-toolchain`; fixture
builds and ordinary builds/tests continue to use `rust-toolchain.toml`.

Reports are written to `target/coverage/`: `html/index.html`, `coverage.json`,
`lcov.info`, `summary.txt`, test/threshold logs, and `provenance.txt`. The provenance
records the commit, working tree status, toolchain, features, test scope, and CI
run URL. `cargo xtask coverage report` regenerates reports after a failed run;
partial reports are diagnostic only. `cargo xtask coverage check` requires a
successful full test run and at least **80% line and compiler-region coverage**. Neither failed tests nor percentages below the
target are treated as passing evidence.

The [coverage workflow](https://github.com/wasmCloud/wasmCloud/actions/workflows/coverage.yml)
runs on Rust pull requests, merge groups, and main pushes, and can be started
manually. Its summary records the measured percentages and test/threshold status.
The `rust-coverage` artifact retains the reports for 90 days, including when the
threshold fails. This workflow is separate from the release gate.

#### OpenSSF Best Practices evidence

The Silver badge's
[`test_statement_coverage80`](https://www.bestpractices.dev/en/criteria/1#test_statement_coverage80)
criterion requires 80% **statement coverage**. cargo-llvm-cov reports LLVM source
regions and lines, not a statement count. Using region coverage as a proxy needs
justification in the badge assessment; line coverage must not be labeled as
statement coverage.

The report includes native workspace code and uses cargo-llvm-cov's default
exclusions for test files, examples, benchmarks, dependencies, and build output.
Generated protobuf bindings under `washlet/generated/` are also excluded. Inline
unit-test modules and test-only helper functions use conditional `coverage(off)`
attributes so their bodies do not count toward the percentage. These attributes
are active only during nightly coverage builds. Platform-specific code is measured
only where compiled on Linux.
Guest Wasm components, the Go operator/gateway, and Kubernetes E2E execution are
outside this Rust report's scope. The Rust report alone does not establish
project-wide compliance; the Go modules need their own statement-coverage
reports, and these scope limits need review in the badge justification.

For evidence, link a successful run for the assessed commit and preserve its
`rust-coverage` artifact before it expires. Include the commit, test results,
metrics, and exclusions with the justification. Installing the tool or setting
an 80% threshold does not demonstrate that the criterion is met.

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

## Dependencies

### Selecting

Before adding a dependency, check whether a crate already in the workspace, or the standard library, does the job. A new dependency must be actively maintained, with recent releases and a maintainer who answers issues. Prefer projects hosted by an open source foundation, such as the Bytecode Alliance (`wasmtime`, `wit-bindgen`, `wasm-tools`), over alternatives.

Every dependency must use a license on the allowlist in [`deny.toml`](deny.toml). A crate outside it needs a per-crate exception in `deny.toml`, agreed in review. Changes to the root `Cargo.toml` and `Cargo.lock` need approval from `@wasmCloud/wash-maintainers`, as set in [`CODEOWNERS`](.github/CODEOWNERS).

### Obtaining

- **Rust** crates come from crates.io. Shared versions are declared once under `[workspace.dependencies]` in the root `Cargo.toml`, and `Cargo.lock` is committed.
- **Go** modules for `runtime-operator` and `runtime-gateway` come from the Go module proxy, and are pinned in each module's `go.sum`.
- **GitHub Actions** are pinned to a full commit SHA, with the version in a trailing comment. `zizmor` lints the workflows.

### Tracking

- **Dependabot** opens weekly grouped update PRs for Cargo, Go modules, GitHub Actions and Docker images. It waits a week after a release before proposing it. For Cargo and Go, the wait is 3 days for a patch release and 14 for a major one. Security updates arrive as their own group, without waiting.
- **`cargo audit`** checks `Cargo.lock` against the RustSec advisory database daily, on every push to `main`, and on pull requests that change a manifest, the lockfile, `deny.toml` or `.cargo/audit.toml`. An advisory that doesn't affect wasmCloud can be ignored in [`.cargo/audit.toml`](.cargo/audit.toml), with a comment saying why it doesn't apply.
- **`cargo deny check licenses`** runs on the same triggers and fails on any license outside the allowlist. It checks every Rust workspace, including standalone examples and templates, against the root license policy and its per-crate exceptions. Standalone workspaces are also checked for WebAssembly targets. First-party crates need license metadata unless they set `publish = false`.
- **Dependency review** checks newly introduced dependencies on pull requests to `main` and merge groups that change Rust or Go dependency files, GitHub Actions, `deny.toml` or its policy in [`.github/dependency-review-config.yml`](.github/dependency-review-config.yml). It blocks known vulnerabilities of any severity in runtime, development, and unknown scopes, and checks licenses against the same allowlist as `deny.toml`. Each crate excepted in `deny.toml` is exempted there too, while `cargo deny` enforces its approved license. When GitHub flags an advisory that `.cargo/audit.toml` ignores, its GHSA ID goes in `allow-ghsas`. GitHub reports missing license data as a warning.

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
[2]: https://github.com/wasmCloud/wasmCloud/issues/new?template=bug_report.yml
[3]: https://github.com/wasmCloud/wasmCloud/issues?q=is%3Aissue+is%3Aopen
