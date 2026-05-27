# Rust examples

Reference WebAssembly components written in **Rust** that demonstrate how
to build, run, and publish with `wash`. Each example is a self-contained
project with its own `Cargo.toml`, `.wash/config.yaml`, and `README.md`.

Looking for examples in other languages? Checkout our docs at [wasmcloud.com/docs/examples/](https://wasmcloud.com/docs/examples/).

## Available examples

| Example                                                            | Description                                                                                                                                |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------ |
| [blobby](./blobby/)                                                | Simple file server demonstrating CRUD operations against the `wasi:blobstore` interface.                                                   |
| [grpc-hello-world](./grpc-hello-world/)                            | gRPC client and server components showing how to make and serve gRPC calls from a wasmCloud component.                                     |
| [otel-config](./otel-config/)                                      | HTTP service instrumented with OpenTelemetry tracing, logs, and metrics via `wasi:otel`, with the OTel `Resource` built from `wasi:config`. |
| [qrcode](./qrcode/)                                                | HTTP service that generates QR codes.                                                                                                      |

## Running an example

From the example directory:

```bash
# fetch wit deps and build the component
wash build
```

`wash build` reads the `build.command` from `.wash/config.yaml`, fetches
the example's WIT dependencies, and produces the component artifact at
`build.component_path`.

To run your component locally:

```bash
# build and deploys your WebAssembly component to a local wasmCloud environment
wash dev
```

We recommend running `wash dev` with `watchexec`. `watchexec` watches the project directory and restarts `wash dev` on every save (`-r` restarts the running command, `-c` clears the screen).

First install watchexec:

```bash
cargo install watchexec-cli
```

Then start your development loop:

```bash
watchexec -c -r 'wash dev'
```

Checkout our [docs](https://wasmcloud.com/docs/wash/developer-guide/) for more information including the [Rust Language Guide](https://wasmcloud.com/docs/wash/developer-guide/language-support/rust/).

## CI

Every example listed above is built, formatted, linted, and on pushes to `main`, published to
`ghcr.io/wasmcloud/components/<name>` by the
[`examples`](../.github/workflows/examples.yml) workflow. The workflow
enforces that any new example directory is wired up to a CI job.
