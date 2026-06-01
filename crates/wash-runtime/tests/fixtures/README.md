# wash-runtime test fixtures

WebAssembly components used by the `wash-runtime` integration tests and
benchmarks. Each subdirectory is a small Rust crate compiled to a wasm
component; the tests load the compiled output from `../wasm/` via
`include_bytes!` (and a few runtime reads).

This directory is its **own** Cargo workspace (see `Cargo.toml` here),
separate from the top-level workspace, so the fixtures can target
`wasm32-wasip2` / `wasm32-wasip1` without dragging those targets into the
host crates.

## Building

Fixtures are built by the `xtask` task runner, **not** by a build script:

```bash
# from the repo root
cargo xtask build-fixtures
```

This compiles every fixture, componentizes the P3 (`wasm32-wasip1`) core
modules with the WASI reactor adapter, and stages the results into
`../wasm/*.wasm` (which is gitignored). Run it once before
`cargo test`/`cargo bench` for `wash-runtime`, and again whenever you change
a fixture's source or WIT.

Fixtures are built by `xtask` rather than a build script so that `cargo test`
doesn't trigger nested `cargo build` invocations (the "cargo building cargo"
problem); the `wash-runtime` build script stays pure-Rust proto codegen. The
build logic lives in `/xtask/src/main.rs`.

## Adding a fixture

1. Create a new crate directory here with its `Cargo.toml`, `src/`, and (if
   it imports wasi interfaces) a `wit/` world.
2. Add it to the `members` list in this directory's `Cargo.toml`.
3. Add its package name to `P2_FIXTURES` or `P3_FIXTURES` in
   `/xtask/src/main.rs`. If its world uses only local interfaces (no wasi
   imports), also add it to `P2_SKIP_SHARED_WIT` so shared WIT deps aren't
   copied in.

Shared WIT dependencies live in `p2-wit-deps/` and `p3-wit-deps/`; `xtask`
copies them into each fixture's `wit/deps/` at build time.
