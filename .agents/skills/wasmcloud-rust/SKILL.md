---
name: wasmcloud-rust
description: Implement, review, or validate Rust changes in wasmCloud, including library APIs, Wasm test fixtures, workspace tests, and Clippy; also find specialist guidance for Wasmtime bug investigations and audits.
---

# Rust changes

Style:

- Avoid single letter aliases and names,
  for example `use std::fmt as f` or `|x| ...` over a non-trivial closure body.
- Keep files under 1000 lines.
  Split a large module semantically rather than at an arbitrary point.
- Use `let ... else` for early returns, to keep the happy path unindented.
- Omit the `get_` prefix on simple getters.

API surface:

- Avoid exposing internals.
  Default to private, then `pub(crate)`, and make an item `pub` only when it is part of the intended API.
- Keep struct fields private and expose accessors,
  so that the representation can change without breaking callers.
- Do not expose types from dependencies in a public API.
  Widely used types such as `anyhow::Result` are an exception.
- When changing a Rust library's public API, check for breaking changes with
  [`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks),
  in addition to the usual checks.
  Select the affected crate with `-p <crate>` and compare against the PR's base revision
  with `--baseline-rev <revision>`.
  `wash-runtime` is unpublished, so the default crates.io baseline is not suitable.
  Review CLI, WIT, and other compatibility changes separately.

Errors:

- Use `thiserror` for custom error types that callers match on.
- Use `anyhow` for operational errors that are more widely scoped.

Before committing Rust changes (run from the repository root):

- Use the toolchain pinned in [`rust-toolchain.toml`](../../../rust-toolchain.toml) for builds, tests, and Clippy.
  Use nightly only for formatting, unless a task explicitly requires it.
- Format with `cargo +nightly fmt`.
  CI checks it with `cargo +nightly fmt -- --check`.
- Build the Wasm fixtures with `cargo xtask build-fixtures` before workspace tests or all-targets Clippy.
  Tests and benchmarks embed these generated files at compile time.
  Rebuild them after changing fixtures under `crates/wash-runtime/tests/fixtures/`.
  See [Running Tests](../../../CONTRIBUTING.md#running-tests) for the build prerequisites.
- Set `OPTIONAL_FEATURES` to the value in [`wash.yml`](../../../.github/workflows/wash.yml),
  then run `cargo test --workspace --features "$OPTIONAL_FEATURES"`.
  Plain `cargo test` selects only the default member, `crates/wash`.
  The ignored tests also need Docker and registry access;
  run them with the same command followed by `-- --ignored` when relevant.
- Lint with `cargo clippy --workspace --all-targets`,
  with the same `--features` that [`wash.yml`](../../../.github/workflows/wash.yml) sets in `OPTIONAL_FEATURES`.

Tooling:

- Use `cargo xtask <task>` for repository tasks.

## Specialist guidance

Load a published skill only when its topic is needed and your harness supports it.

- [rust-development](https://github.com/cosmonic-labs/skills/tree/main/rust-development):
  Rust naming, types, errors, API design, and documentation.
- [reduce-test-cases](https://github.com/bytecodealliance/wasmtime/tree/main/.agents/skills/reduce-test-cases):
  minimizing a failing `.wasm`, `.wat`, `.wast`, or `.clif` input before reporting a Wasmtime bug.
- [wasmtime-auditor](https://github.com/bytecodealliance/wasmtime/tree/main/.agents/skills/wasmtime-auditor):
  auditing Wasmtime for bugs and vulnerabilities when the task calls for that investigation.
