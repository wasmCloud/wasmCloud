# HTTP KeyValue Counter

This is a simple Rust Wasm example that increments a key in a keyvalue store in response to an HTTP request based on the path. This component uses the [wasi-http](https://github.com/WebAssembly/wasi-http) API to receive HTTP requests and the [wasi-keyvalue](https://github.com/WebAssembly/wasi-keyvalue) API to interact with a keyvalue store. At runtime we [link](https://wasmcloud.com/docs/1.0/concepts/linking-components) this component to an implementation of wasi-keyvalue that interacts with [Redis](https://redis.io/).

## Prerequisites

- `cargo` 1.75
- [`wash`](https://wasmcloud.com/docs/installation) 0.27.0

## Building

```bash
wash build
```

## Running with wasmCloud

Ensuring you've built your component with `wash build`, you can launch wasmCloud and deploy the full hello world application with the following commands. Once the application reports as **Deployed** in the application list, you can use `curl` to send a request to the running HTTP server.

```shell
# Start redis locally
redis-server &
# Start wasmCloud
wash up -d
wash app deploy ./wadm.yaml
wash app list
curl http://127.0.0.1:8080/counter
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.
