# HTTP Hello World

This is a simple TinyGo Wasm example that responds with a "Hello World" message for each request.

## Prerequisites

- `go` 1.23
- `tinygo` 0.33
- [`wash`](https://wasmcloud.com/docs/installation) 0.35.0
- `wasmtime` 25.0.0 (if running with wasmtime)

## Building

```bash
wash build
```

## Running with wasmtime

You must have wasmtime 25.0.0 for this to work. Make sure to follow the build step above first.

```bash
wasmtime serve -Scommon ./build/http_hello_world_s.wasm
```

## Running with wasmCloud

Make sure to follow the build steps above, and replace the file path in [the wadm manifest](./wadm.yaml) with the absolute path to your local built component.

```shell
wash up -d
wash app deploy ./wadm.yaml
curl http://localhost:8000
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=tinygo) section of the wasmCloud documentation.
