# Python Examples

This folder contains examples that can be run with [wasmcloud][wasmcloud] which are written in [Python][python].

While WebAssembly was initially developed as a Web-centered abstraction, work is underway to make WebAssembly usable and convenient from server-side Javascript.

Work is still ongoing, but the WebAssembly-for-Python effort is primarily driven forward by [`componentize-py`][componentize-py].

[componentize-py]: https://github.com/bytecodealliance/componentize-py/

## Building individual example components

Example components built in Python can be compiled similarly to any other wasmcloud project:

```console
wash build
```

While Python is not yet officially supported by `wash`, `wash`'s custom `build_command` support makes it possible to build Python projects to run on wasmcloud.

## WebAssembly support

As WebAssembly is intended to be a "compilation target" for native Python code, upstream work is underway to integrate and improve support for the various standards of WebAssembly.

| Language | Core Modules (`wasm32-unknown-unknown`)                                     | Preview 1 (`wasm32-wasi-preview1`) | WASI Preview 2 (`wasm32-wasi-preview2`) |
| -------- | --------------------------------------------------------------------------- | ---------------------------------- | --------------------------------------- |
| Python   | [cpython](https://github.com/python/cpython/blob/main/Tools/wasm/README.md) | ❌                                 | ✅ (`componentize-py componentize ...`) |

> [!NOTE]
> Don't know what `wasm32-unknown-unknown` means versus `wasm32-wasi-preview1`?
>
> `wasm32-unknown-unknown` is a compile target which deals in core [WebAssembly modules][wasm-core-modules] (i.e. you're only given access to numbers at this level)
> [`wasm32-wasi-preview1`][wasi-p1] is a compile target that provides richer types, support for more higher level platform APIs
> [`wasm32-wasi-preview2`][wasi-p2] is the next generation compile target with much richer types, higher level APIs like async, streaming, the WIT IDL.
>
> In a sentence, WebAssembly functionality is layered, with `wasm32-unknown-unknown` being the most basic (only doing operations on numbers) and `wasm32-wasi-preview2` being the current most advanced.

## Want to learn more?

To learn more, check out the [SIG Guest Languages Zulip group](https://bytecodealliance.zulipchat.com/#narrow/stream/394175-SIG-Guest-Languages).

Development on Python support is stewarded by the [Bytecode Alliance][bca].

Work on [`componentize-py`][componentize-py] are done in the open, and you are welcome to try out the toolchain, contribute, and ask questions.

[python]: https://www.python.org/
[wasmcloud]: https://wasmcloud.com
[wasi-p1]: https://github.com/WebAssembly/WASI/blob/main/legacy/preview1/docs.md
[wasi-p2]: https://github.com/WebAssembly/WASI/blob/main/preview2
[wasi-p2-adapter]: https://github.com/bytecodealliance/wasmtime/tree/main/crates/wasi-preview1-component-adapter
[wasm-core-modules]: https://webassembly.github.io/spec/core/
[bca]: https://bytecodealliance.org/
[wasmtime]: https://github.com/bytecodealliance/wasmtime
