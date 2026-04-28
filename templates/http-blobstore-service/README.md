# http-blobstore-service

A WebAssembly HTTP service component that stores and retrieves blobs through the `wasi:blobstore` interface, with routing provided by [`wstd-axum`](https://docs.rs/wstd-axum).

## What it does

| Method   | Path     | Behavior |
|----------|----------|----------|
| `GET`    | `/`      | Lists all keys in the container as JSON |
| `GET`    | `/<key>` | Returns the bytes stored at `<key>` |
| `PUT`    | `/<key>` | Writes the request body to `<key>` |
| `DELETE` | `/<key>` | Removes `<key>` from the container |

The component itself has no knowledge of the underlying storage. It speaks only the `wasi:blobstore` interface; the host runtime wires that interface to a backend (filesystem, NATS object store, etc.) based on `.wash/config.yaml`.

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
# Store a blob
curl -X PUT --data-binary @somefile.txt http://localhost:8000/notes.txt

# List keys
curl http://localhost:8000/

# Read a blob
curl http://localhost:8000/notes.txt

# Delete a blob
curl -X DELETE http://localhost:8000/notes.txt
```

## Choosing a backend

Open [`.wash/config.yaml`](./.wash/config.yaml) and uncomment the section for your preferred backend. The default is the filesystem-backed blobstore in a `wash dev` environment.

## Building manually

```bash
cargo build --target wasm32-wasip2 --release
```

The compiled component is written to `target/wasm32-wasip2/release/http_blobstore_service.wasm`.
