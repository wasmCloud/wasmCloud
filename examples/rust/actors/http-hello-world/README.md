# HTTP Hello World

This is a simple Rust Wasm example that responds with a "Hello World" message for each request.

## Prerequisites

- `cargo` 1.74
- `wash` 0.25.0
- `wasmtime` 16.0.0 (if running with wasmtime)

## Building

```bash
wash build
```

## Running with wasmtime

You must have wasmtime 16.0.0 for this to work. Make sure to follow the build step above first.

```bash
wasmtime serve -Scommon ./build/http_hello_world_s.wasm
```

## Running with wasmCloud

Make sure to follow the build steps above, and replace the file path in [the wadm manifest](./wadm.yaml) with the absolute path to your local built component.

```
wash up -d
wash app deploy ./wadm.yaml
curl http://localhost:8081
```
