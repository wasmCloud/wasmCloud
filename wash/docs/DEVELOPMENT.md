# Developer guide

This document serves as a guide and reference for people looking to develop `wash`.

- [Developer guide](#developer-guide)
  - [Development Prerequistes](#development-prerequistes)
    - [`build` Integration Tests](#build-integration-tests)
  - [Creature comforts](#creature-comforts)
  - [Building the project](#building-the-project)
  - [Testing the project](#testing-the-project)

## Development Prerequistes

To contribute to `wash`, you just need [Rust](https://rustup.rs/) installed.

### `build` Integration Tests

To run the `wash build` integration tests that compile actors using actual language toolchains, you must have those toolchains installed. Currently the requirements for this are:

- [Rust](https://rustup.rs/)
  - The `wasm32-unknown-unknown` target must be installed.
    - You can install this with: `rustup target add wasm32-unknown-unknown`.
- [TinyGo](https://tinygo.org/getting-started/install/)
  - TinyGo also requires [Go](https://go.dev/doc/install) to be installed.

To make it easy to ensure you have all the right tools installed to run all `wash` tests, we've created a Python script at `tools/deps_check.py`. You can run this using `make deps-check` or `python3 ./tools/deps_check.py`.

## Creature comforts

While developing `wash`, consider installing the following:

- [`cargo-watch`][cargo-watch] (`cargo install cargo-watch`) to enable the `*-watch` commands

[cargo-watch]: https://crates.io/crates/cargo-watch

## Building the project

To build the project:

```console
make build
```

To build continuously (thanks to [`cargo-watch`][cargo-watch]):

```console
make build-watch
```

## Testing the project

To test the project:

```console
make test
make test-unit
```

To test the project continuously:

```console
make test-watch
```

To test a *specific* unit test continuously:

```console
TARGET=integration_new_handles_dashed_names make test-unit-watch
```
