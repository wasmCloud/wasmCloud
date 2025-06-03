# HTTP Hello World

This is a simple C Wasm example that responds with a "Hello World" message for each request.

## Prerequisites

- `cargo` 1.75
- [`wash`](https://wasmcloud.com/docs/installation) 0.27.0
- `wasmtime` >=19.0.0 (if running with wasmtime)
- [wasi-sdk](https://github.com/WebAssembly/wasi-sdk)
- [wit-bindgen](https://github.com/bytecodealliance/wit-bindgen)

## Building

This example requires the wasi-sdk. The variable `WASI_SDK_DIR` defaults to `/opt/wasi-sdk`.
```bash
wash build
```

## Running with wasmtime

You must have wasmtime >=19.0.0 for this to work. Make sure to follow the build step above first.

```bash
wasmtime serve -Scommon ./http_hello_world_s.wasm
```

## Running with wasmCloud

Ensuring you've built your component with `wash build`, you can launch wasmCloud and deploy the full hello world application with the following commands. Once the application reports as **Deployed** in the application list, you can use `curl` to send a request to the running HTTP server.

```shell
wash up -d
wash app deploy ./wadm.yaml
wash app get
curl http://127.0.0.1:8000
```
