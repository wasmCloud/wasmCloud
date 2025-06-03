# HTTP JSONify

This is a simple Rust Wasm example that converts an incoming HTTP request to a JSON representation, and returns that as the response.

## Prerequisites

- `cargo` >=1.75
- [`wash`](https://wasmcloud.com/docs/installation) >=0.27.0
- `wasmtime` >=19.0.0 (if running with wasmtime)

## BUilding

```console
wash build
```

## Running with wasmtime

You must have wasmtime >=19.0.0 for this to work. Make sure to follow the build step above first.

```bash
wasmtime serve -Scommon ./build/http_hello_world_s.wasm
```

## Running with wasmCloud

Ensuring you've built your component with `wash build`, you can launch wasmCloud and deploy the full hello world application with the following commands.

Once the application reports as **Deployed** in the application list, you can use `curl` to send a request to the running HTTP server.

```shell
wash up -d
wash app deploy ./wadm.yaml
wash app get
curl http://127.0.0.1:8000
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.
