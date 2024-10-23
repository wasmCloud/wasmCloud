# Echo Messaging

This is a simple Rust Wasm example that uses `wasmcloud:messaging` to echo back any message it receives.

## Prerequisites

- `cargo` 1.75
- [`wash`](https://wasmcloud.com/docs/installation) 0.27.0

## Building

```bash
wash build
```

## Running with wasmCloud

Ensuring you've built your component with `wash build`, you can launch wasmCloud and deploy the full hello world application with the following commands. Once the application reports as **Deployed** in the application list, you can use the [NATS CLI](https://github.com/nats-io/natscli) to send it a request.

```shell
wash up -d
wash app deploy ./wadm.yaml
wash app get
nats req "wasmcloud.echo" "HELLLLOOOOOO"
```

And in response:

```shell
HELLLLOOOOOO
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.
