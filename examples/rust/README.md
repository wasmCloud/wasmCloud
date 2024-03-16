# Rust Examples

This folder contains examples that can be run with [wasmcloud][wasmcloud] which are written in [Rust][rust].

As much of WebAssembly tooling is written in Rust, Rust currently posseses the best support for WebAssembly and it's emerging standards.

## Building individual example components

Example components built in Rust can be compiled similarly to any other wasmcloud project:

```console
wash build
```

## WebAssembly support

As WebAssembly is intended to be a "compilation target" for native Rust code, upstream work is underway to integrate and improve support for the various standards of WebAssembly.

| Language | Core Modules (`wasm32-unknown-unknown`) | Preview 1 (`wasm32-wasi-preview1`) | WASI Preview 2 (`wasm32-wasi-preview2`)                                              |
|----------|-----------------------------------------|------------------------------------|--------------------------------------------------------------------------------------|
| Rust     | ✅ (`--target=wasm32-unknown-unknown`)  | ✅ (`--target=wasm32-wasi`)        | ✅ (requires [adapter][wasi-p2-adapter],  [`rustc` Github Issue][rust-wasip2-issue]) |

> [!NOTE]
> Don't know what `wasm32-unknown-unknown` means versus `wasm32-wasi-preview1`?
>
> `wasm32-unknown-unknown` is a compile target which deals in core [WebAssembly modules][wasm-core-modules] (i.e. you're only given access to numbers at this level)
> [`wasm32-wasi-preview1`][wasi-p1] is a compile target that provides richer types, support for more higher level platform APIs
> [`wasm32-wasi-preview2`][wasi-p2] is the next generation compile target with much richer types, higher level APIs like async, streaming, the WIT IDL.
>
> In a sentence, WebAssembly functionality is layered, with `wasm32-unknown-unknown` being the most basic (only doing operations on numbers) and `wasm32-wasi-preview2` being the current most advanced.

[rust-wasip2-issue]: https://github.com/rust-lang/rust/pull/119616

## Want to learn more?

To learn more, check out the [SIG Guest Languages Zulip group](https://bytecodealliance.zulipchat.com/#narrow/stream/394175-SIG-Guest-Languages).

Development on Rust support is stewarded by the [Bytecode Alliance][bca].

The Bytecode Alliance maintains a WebAssembly runtime called [`wasmtime`][wasmtime] which is written in Rust, and is a great place to start learning about Rust support.

[rust]: https://rust-lang.org
[wasmcloud]: https://wasmcloud.com
[wasi-p1]: https://github.com/WebAssembly/WASI/blob/main/legacy/preview1/docs.md
[wasi-p2]: https://github.com/WebAssembly/WASI/blob/main/preview2
[wasi-p2-adapter]: https://github.com/bytecodealliance/wasmtime/tree/main/crates/wasi-preview1-component-adapter
[wasm-core-modules]: https://webassembly.github.io/spec/core/
[bca]: https://bytecodealliance.org/
[wasmtime]: https://github.com/bytecodealliance/wasmtime
