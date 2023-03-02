# Developer guide

This document serves as a guide and reference for people looking to develop `wash`.

- [Developer guide](#developer-guide)
  - [Development Prerequistes](#development-prerequistes)
    - [`build` Integration Tests](#build-integration-tests)
    - [Dependency Check Script](#dependency-check-script)
    - [Optional Tools](#optional-tools)
  - [Building the project](#building-the-project)
  - [Testing the project](#testing-the-project)

## Development Prerequistes

To contribute to `wash`, you just need [Rust](https://rustup.rs/) installed. To run any `wash` tests, you need to install [`nextest`](https://nexte.st/index.html). With a Rust toolchain already installed, you can simply install this with:

```bash
cargo install cargo-nextest --locked
```

The dependency check script will also install this for you, see that section below.

### `build` Integration Tests

To run the `wash build` integration tests that compile actors using actual language toolchains, you must have those toolchains installed. Currently the requirements for this are:

- [Rust](https://rustup.rs/)
  - The `wasm32-unknown-unknown` target must be installed.
    - You can install this with: `rustup target add wasm32-unknown-unknown`.
- [TinyGo](https://tinygo.org/getting-started/install/)
  - TinyGo also requires [Go](https://go.dev/doc/install) to be installed.

### Dependency Check Script

To make it easy to ensure you have all the right tools installed to run all of the `wash` tests, we've created a Python script at `tools/deps_check.py`. You can run this using `make deps-check` or `python3 ./tools/deps_check.py`.

### Optional Tools

While developing `wash`, consider installing the following optional development tools:

- [`cargo-watch`](https://crates.io/crates/cargo-watch) (`cargo install cargo-watch`) to enable the `*-watch` commands

These will be automatically installed using the `deps_check.py` script as well.

## Building the project

To build the project:

```console
make build
```

To build continuously (thanks to [`cargo-watch`](https://crates.io/crates/cargo-watch)):

```console
make build-watch
```

## Testing the project

To test all unit tests:

```console
make test
```

To test all unit tests continuously:

```console
make test-watch
```

To test a *specific* target test(s) continuously:

```console
TARGET=integration_new_handles_dashed_names make test-watch
```
