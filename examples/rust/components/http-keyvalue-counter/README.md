# HTTP KeyValue Counter

This is a simple Rust Wasm example that increments a key in a keyvalue store in response to an HTTP request based on the path. This component uses the [wasi-http](https://github.com/WebAssembly/wasi-http) API to receive HTTP requests and the [wasi-keyvalue](https://github.com/WebAssembly/wasi-keyvalue) API to interact with a keyvalue store. At runtime we [link](https://wasmcloud.com/docs/concepts/linking-components) this component to an implementation of wasi-keyvalue that interacts with [Redis](https://redis.io/).

## Prerequisites

- `cargo` 1.75
- [`wash`](https://wasmcloud.com/docs/installation) 0.36.1

## Building

```bash
wash build
```

## Running with wasmCloud

You can build and deploy your component with all dependencies and a hot reload loop with `wash dev`.

```shell
wash dev
```

Then, you can `curl` your HTTP handler.

```shell
curl http://127.0.0.1:8000/counter
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.
