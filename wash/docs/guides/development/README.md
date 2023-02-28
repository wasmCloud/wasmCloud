# Developer guide

This document serves as a guide and reference for people looking to develop `wash`.

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
