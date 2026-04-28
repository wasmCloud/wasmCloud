# HTTP Client in Rust

This project template is a WebAssembly component built with [Rust][rust] that demonstrates making outgoing HTTP requests using [`wstd::http::Client`][wstd-client].

It uses [`wstd`][wstd]'s `#[http_server]` proc macro to handle the incoming request and proxies the call upstream.

[rust]: https://www.rust-lang.org/
[wstd]: https://github.com/bytecodealliance/wstd
[wstd-client]: https://docs.rs/wstd/latest/wstd/http/struct.Client.html

## Prerequisites

- [Wasm Shell (`wash`)][wash]
- [Rust toolchain][rust-install]
- The `wasm32-wasip2` Rust target: `rustup target add wasm32-wasip2`

[wash]: https://wasmcloud.com/docs/installation
[rust-install]: https://www.rust-lang.org/tools/install

## Local development

Use `wash new` to scaffold a new wasmCloud component project:

```shell
wash new https://github.com/wasmCloud/wasmCloud.git --name http-client --subfolder templates/http-client
```

```shell
cd http-client
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

The component proxies a `GET https://httpbin.org/get` and returns the upstream JSON.

## Customizing the upstream

Change the `UPSTREAM_URL` constant in [src/lib.rs](src/lib.rs) to point at any HTTPS endpoint. Outgoing TLS is provided by the host runtime and no additional crates are required in the component.

## Build Wasm binary

```shell
wash build
```

## WIT Interfaces

This component imports and exports the following [WIT interfaces](https://component-model.bytecodealliance.org/design/wit.html):

```wit
world http-client {
  import wasi:http/outgoing-handler@0.2.2;
  export wasi:http/incoming-handler@0.2.2;
}
```
