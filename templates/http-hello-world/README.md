# HTTP Hello World in Rust

A minimal WebAssembly component built with [Rust][rust] that responds to HTTP requests using the [`wstd`][wstd] async standard library and its `#[http_server]` proc macro.

[rust]: https://www.rust-lang.org/
[wstd]: https://github.com/bytecodealliance/wstd

## Prerequisites

- [Wasm Shell (`wash`)][wash]
- [Rust toolchain][rust-install]
- The `wasm32-wasip2` Rust target: `rustup target add wasm32-wasip2`

[wash]: https://wasmcloud.com/docs/installation
[rust-install]: https://www.rust-lang.org/tools/install

## Local development

Use `wash new` to scaffold a new wasmCloud component project:

```shell
wash new https://github.com/wasmCloud/wasmCloud.git --name http-hello-world --subfolder templates/http-hello-world
```

```shell
cd http-hello-world
```

To build this project and run in a hot-reloading development loop, run `wash dev` from this directory:

```shell
wash dev
```

### Send a request to the running component

Once `wash dev` is serving your component, send a request:

```shell
curl localhost:8000
```

```text
Hello from wasmCloud!
```

## Build Wasm binary

```shell
wash build
```

## WIT Interfaces

This component exports the following [WIT interfaces](https://component-model.bytecodealliance.org/design/wit.html):

```wit
world hello {
  export wasi:http/incoming-handler@0.2.2;
}
```
