# http-service

A WebAssembly HTTP service component using [`axum`](https://docs.rs/axum) for routing, wired to `wasi:http/incoming-handler` via [`wstd-axum`](https://docs.rs/wstd-axum).

## What it does

| Method | Path | Behavior |
|---|---|---|
| `GET`  | `/`               | Returns a hello message |
| `GET`  | `/api/greet?name=...` | Returns `Hello, <name>!` (defaults to `world`) |
| `POST` | `/api/echo`       | Echoes the JSON body back |

## Why `wstd-axum` and not `axum` directly

Standard `axum` runs on `tokio` + `hyper`. Inside a Wasm component, HTTP is provided as a host-imported interface (`wasi:http`), so neither `tokio`'s networking stack nor `hyper`'s socket layer applies. `wstd-axum` adapts an `axum::Router` onto the `wasi:http/incoming-handler` export instead. It is configured in `Cargo.toml` with `axum = { default-features = false, ... }` and only the framework features that do not require `tokio` or `hyper`.

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
curl http://localhost:8000/
curl 'http://localhost:8000/api/greet?name=Eric'
curl -X POST http://localhost:8000/api/echo \
  -H 'Content-Type: application/json' \
  -d '{"message":"hello"}'
```

## Building manually

```bash
cargo build --target wasm32-wasip2 --release
```

The compiled component is written to `target/wasm32-wasip2/release/http_service.wasm`.
