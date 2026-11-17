---
name: wasmcloud-rust
description: Rust conventions and pre-commit checks for wasmCloud, covering style, public API surface, error types, Wasm test fixtures, workspace tests, Clippy, cargo machete, and cargo-semver-checks. Use when changing or reviewing .rs files, Cargo.toml or Cargo.lock, crates/, xtask/, or fuzz/, or when investigating or reporting a Wasmtime bug.
---

# Rust changes

Style:

- Avoid single letter aliases and names,
  for example `use std::fmt as f` or `|x| ...` over a non-trivial closure body.
- Keep new files under about 1000 lines.
  Split a large module semantically rather than at an arbitrary point.
  Do not split an existing large file as part of an unrelated change.
- Use `let ... else` for early returns, to keep the happy path unindented.
- Omit the `get_` prefix on simple getters.

API surface:

- Avoid exposing internals.
  Default to private, then `pub(crate)`, and make an item `pub` only when it is part of the intended API.
- Keep struct fields private and expose accessors,
  so that the representation can change without breaking callers.
- Do not expose types from dependencies in a new public API.
  Widely used types such as `anyhow::Result` are an exception,
  as is `wash-runtime`'s existing re-export of `wasmtime`.
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

## Before committing

The `lint` and `check` jobs in [`wash.yml`](../../../.github/workflows/wash.yml) are the source of truth.
If a command below differs from the workflow, follow the workflow.
Run everything from the repository root,
with the toolchain pinned in [`rust-toolchain.toml`](../../../rust-toolchain.toml).
Use nightly only for formatting, unless a task explicitly requires it.

Copy this checklist and work through it:

```
- [ ] Set OPTIONAL_FEATURES to the value in wash.yml
- [ ] cargo xtask build-fixtures
- [ ] cargo +nightly fmt
- [ ] cargo clippy --workspace --features "$OPTIONAL_FEATURES"
- [ ] cargo machete
- [ ] The anyhow context casing step from the wash.yml lint job
- [ ] node .github/scripts/wasmtime-features.mjs
- [ ] cargo test --workspace --features "$OPTIONAL_FEATURES"
```

If any step fails, fix the cause and re-run the checklist from that step until every step passes.

Notes on the steps:

- Tests and benchmarks embed the Wasm fixtures at compile time,
  so build them before tests or Clippy, and rebuild after changing `crates/wash-runtime/tests/fixtures/`.
  See [Running Tests](../../../CONTRIBUTING.md#running-tests) for the build prerequisites.
- Plain `cargo test` selects only the default member, `crates/wash`.
- The ignored tests also need Docker and registry access;
  run them with the same test command followed by `-- --ignored` when relevant.
- The casing step requires `.context()` and `.with_context()` messages under `crates/*/src` to start lowercase.

## Specialist guidance

Load a published skill only when its topic is needed and your harness supports it.

- [rust-development](https://github.com/cosmonic-labs/skills/tree/main/rust-development):
  Rust naming, types, errors, API design, and documentation.
- [reduce-test-cases](https://github.com/bytecodealliance/wasmtime/tree/main/.agents/skills/reduce-test-cases):
  minimizing a failing `.wasm`, `.wat`, `.wast`, or `.clif` input before reporting a Wasmtime bug.
- [wasmtime-auditor](https://github.com/bytecodealliance/wasmtime/tree/main/.agents/skills/wasmtime-auditor):
  auditing Wasmtime for bugs and vulnerabilities when the task calls for that investigation.
