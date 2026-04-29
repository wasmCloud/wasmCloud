# HTTP Handler in Rust

This project template is a WebAssembly component built with [Rust][rust] that demonstrates an HTTP handler with routing, query parameters, and JSON request and response bodies.

The component uses [`axum`][axum] for routing, wired to `wasi:http/incoming-handler` via [`wstd-axum`][wstd-axum]. Standard `axum` runs on `tokio` plus `hyper`. Inside a Wasm component, HTTP arrives through the host-imported `wasi:http` interface, so neither tokio's networking stack nor hyper's socket layer applies. `wstd-axum` adapts an `axum::Router` onto the `wasi:http/incoming-handler` export. The `Cargo.toml` opts out of axum's default features and only enables those that do not require tokio or hyper.

[rust]: https://www.rust-lang.org/
[axum]: https://docs.rs/axum
[wstd-axum]: https://docs.rs/wstd-axum

## Prerequisites

- [Wasm Shell (`wash`)][wash]
- [Rust toolchain][rust-install]
- The `wasm32-wasip2` Rust target: `rustup target add wasm32-wasip2`

[wash]: https://wasmcloud.com/docs/installation
[rust-install]: https://www.rust-lang.org/tools/install

## Local development

Use `wash new` to scaffold a new wasmCloud component project:

```shell
wash new https://github.com/wasmCloud/wasmCloud.git --name http-handler --subfolder templates/http-handler
```

```shell
cd http-handler
```

To build this project and run in a hot-reloading development loop, run `wash dev` from this directory:

```shell
wash dev
```

## Endpoints

| Endpoint | Method | Description |
| -------- | ------ | ----------- |
| `/` | GET | Returns a hello message |
| `/api/greet` | GET | Returns `Hello, <name>!` (uses the `name` query parameter, defaults to `world`) |
| `/api/echo` | POST | Echoes the JSON request body |

## Send requests to the running component

```shell
curl http://localhost:8000/
curl 'http://localhost:8000/api/greet?name=Eric'
curl -X POST http://localhost:8000/api/echo \
  -H 'Content-Type: application/json' \
  -d '{"message":"hello"}'
```

## Build Wasm binary

```shell
wash build
```

## WIT Interfaces

This component exports the following [WIT interfaces](https://component-model.bytecodealliance.org/design/wit.html):

```wit
world http-handler {
  export wasi:http/incoming-handler@0.2.2;
}
```
