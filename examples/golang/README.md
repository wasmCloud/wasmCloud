# Golang Examples

This folder contains examples that can be run with [wasmcloud][wasmcloud] which are written in [Golang][golang].

Most examples compile with [TinyGo][tinygo] rather than the Golang compiler proper given that WebAssembly support landed in TinyGo first and is still slightly better supported there.

## Building individual example components

Example components built in Golang can be compiled similarly to any other wasmcloud project:

```console
wash build
```

## WebAssembly support

As WebAssembly is intended to be a "compilation target" for native Golang code, upstream work is underway to integrate and improve support for the various standards of WebAssembly.

| Language | Core Modules (`wasm32-unknown-unknown`) | Preview 1 (`wasm32-wasi-preview1`) | WASI Preview 2 (`wasm32-wasi-preview2`)  |
|----------|-----------------------------------------|------------------------------------|------------------------------------------|
| Golang   | ✅ (`GOOS=js`,`GOARCH=wasm`)            | ✅ (`GOOS=wasip1`)                 | ✅ (requires [adapter][wasi-p2-adapter]) |
| TinyGo   | ✅ (`-target=wasm`)                     | ✅ (`-target=wasi`)                | ✅ (requires [adapter][wasi-p2-adapter]) |

Tiny Go WASI support is detailed [on their website][tinygo-wasi].
Golang WASI preview 1 support was [announced on the Golang blog][golang-blog-wasi]

> [!NOTE]
> Don't know what `wasm32-unknown-unknown` means versus `wasm32-wasi-preview1`?
>
> `wasm32-unknown-unknown` is a compile target which deals in core [WebAssembly modules][wasm-core-modules] (i.e. you're only given access to numbers at this level)
> [`wasm32-wasi-preview1`][wasi-p1] is a compile target that provides richer types, support for more higher level platform APIs
> [`wasm32-wasi-preview2`][wasi-p2] is the next generation compile target with much richer types, higher level APIs like async, streaming, the WIT IDL.
>
> In a sentence, WebAssembly functionality is layered, with `wasm32-unknown-unknown` being the most basic (only doing operations on numbers) and `wasm32-wasi-preview2` being the current most advanced.

## Want to learn more?

To learn more, check out the [SIG Guest Languages Zulip group](https://bytecodealliance.zulipchat.com/#narrow/stream/394175-SIG-Guest-Languages) and the [issue noting `go` support](https://github.com/bytecodealliance/governance/issues/72).

Development on Golang support (along with Tinygo support) is stewarded by the [Bytecode Alliance][bca].

[golang]: https://golang.org
[wasmcloud]: https://wasmcloud.com
[tinygo-wasi]: https://tinygo.org/docs/guides/webassembly/wasi/
[golang-blog-wasi]: https://tip.golang.org/blog/wasi
[wasi-p1]: https://github.com/WebAssembly/WASI/blob/main/legacy/preview1/docs.md
[wasi-p2]: https://github.com/WebAssembly/WASI/blob/main/preview2
[wasi-p2-adapter]: https://github.com/bytecodealliance/wasmtime/tree/main/crates/wasi-preview1-component-adapter
[wasm-core-modules]: https://webassembly.github.io/spec/core/
[bca]: https://bytecodealliance.org/
[tinygo]: https://tinygo.org
