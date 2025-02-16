# Developer guide

This document serves as a guide and reference for people looking to develop `wash`.

- [Developer guide](#developer-guide)
  - [Development Prerequisites](#development-prerequisites)
    - [`build` Integration Tests](#build-integration-tests)
    - [Dependency Check Script](#dependency-check-script)
    - [Optional Tools](#optional-tools)
  - [Building the project](#building-the-project)
  - [Testing Local Changes](#testing-local-changes)
  - [Testing the project](#testing-the-project)
  - [Making Commits](#making-commits)

## Development Prerequisites

To contribute to `wash`, you just need [Rust](https://rustup.rs/) installed. To run any `wash` tests, you need to install [`nextest`](https://nexte.st/index.html). With a Rust toolchain already installed, you can simply install this with:

```bash
cargo install cargo-nextest --locked
```

The dependency check script will also install this for you, see that section below.

### `build` Integration Tests

To run the `wash build` integration tests that compile components using actual language toolchains, you must have those toolchains installed. Currently the requirements for this are:

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
## Testing Local Changes

After making changes to code in `crates/wash`, you can build and run a new version of `wash` similarly to any other Rust binary project with `cargo run`:

```console
cd crates/wash
cargo run -- <args>
```
For example : `cargo run -- up` is equivalent to `wash up`, with the latest code.

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

## Making Commits

For us to be able to merge in any commits, they need to be signed off. If you didn't do so, the PR bot will let you know how to fix it, but it's worth knowing how to do it in advance.

There are a few options:
- use `git commit -s` in the CLI
- in `vscode`, go to settings and set the `git.alwaysSignOff` setting. Note that the dev container configuration in this repo sets this up by default.
- manually add "Signed-off-by: NAME <EMAIL>" at the end of each commit

You may also be able to use GPG signing in lieu of a sign off.
