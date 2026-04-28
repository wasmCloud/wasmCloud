# http-client

A WebAssembly component that handles incoming HTTP requests by making outgoing HTTP calls to an upstream service. Demonstrates the `wasi:http/outgoing-handler` import alongside the standard `wasi:http/incoming-handler` export.

## What it does

For every incoming request, the component sends a `GET` to the URL in `UPSTREAM_URL` (default: `https://httpbin.org/get`) and returns the upstream response body and status code.

## Prerequisites

- Rust with the `wasm32-wasip2` target: `rustup target add wasm32-wasip2`
- The `wash` CLI

## Running

```bash
wash dev
```

This builds the component and starts an HTTP server on `http://localhost:8000`.

## Usage

```bash
curl http://localhost:8000
```

The response body is the JSON `httpbin.org` returns for the upstream `GET`.

## Customizing the upstream

Change the `UPSTREAM_URL` constant in [src/lib.rs](src/lib.rs) to point at any HTTPS endpoint. Outgoing TLS is provided by the host runtime; no additional crates are required in the component.

## Building manually

```bash
cargo build --target wasm32-wasip2 --release
```

The compiled component is written to `target/wasm32-wasip2/release/http_client.wasm`.
