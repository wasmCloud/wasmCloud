# http-kv-service

A WebAssembly HTTP server component that stores and retrieves key-value pairs using any backend you prefer via the `wasi:keyvalue` interface.

## What it does

- `POST /` — accepts a JSON body `{"key":"...","value":"..."}` and stores the pair
- `GET /?key=<key>` — returns the stored value, or `404` if the key does not exist

The component itself has no knowledge of the underlying storage. It speaks only the `wasi:keyvalue/store` interface; the host runtime wires that interface to the backend you configure.

## Prerequisites

- Rust with the `wasm32-wasip2` target: `rustup target add wasm32-wasip2`
- The `wash` CLI

## Choosing a backend

The `BACKEND` constant in [src/lib.rs](src/lib.rs) controls which bucket name is passed to `open()`. The host runtime selects the actual storage backend based on [.wash/config.yaml](.wash/config.yaml).

Set `BACKEND` to one of the following values and uncomment the matching section in `.wash/config.yaml`:

| `BACKEND`      | Description                                | Required config key            | Example value                    |
|----------------|--------------------------------------------|--------------------------------|----------------------------------|
| `"in_memory"`  | Ephemeral in-process store (default)       | *(none)*                       | —                                |
| `"filesystem"` | Persists data to a local directory         | `wasi_keyvalue_path`           | `/tmp/keyvalue-store`            |
| `"nats"`       | Uses NATS JetStream as the store           | `wasi_keyvalue_nats_url`       | `nats://127.0.0.1:4222`          |
| `"redis"`      | Uses a Redis server as the store           | `wasi_keyvalue_redis_url`      | `redis://127.0.0.1:6379`         |

### in_memory (default)

No configuration required. Data lives only for the lifetime of the `wash dev` process.

```rust
// src/lib.rs
const BACKEND: &str = "in_memory";
```

```yaml
# .wash/config.yaml — no extra keys needed
dev: {}
```

### filesystem

Data is written to a local directory and survives process restarts.

```rust
// src/lib.rs
const BACKEND: &str = "filesystem";
```

```yaml
# .wash/config.yaml
dev:
  wasi_keyvalue_path: /tmp/keyvalue-store
```

### nats

Requires a running NATS server with JetStream enabled.

```rust
// src/lib.rs
const BACKEND: &str = "nats";
```

```yaml
# .wash/config.yaml
dev:
  wasi_keyvalue_nats_url: nats://127.0.0.1:4222
```

Start a local NATS server with JetStream:

```bash
docker run --name nats -p 4222:4222 nats:latest -js
```

### redis

Requires a running Redis server.

```rust
// src/lib.rs
const BACKEND: &str = "redis";
```

```yaml
# .wash/config.yaml
dev:
  wasi_keyvalue_redis_url: redis://127.0.0.1:6379
```

Start a local Redis server:

```bash
docker run --name redis -p 6379:6379 redis:latest
```

## Running

```bash
wash dev
```

This builds the component and starts an HTTP server on `http://localhost:8000`.

## Usage

Store a value:

```bash
curl -X POST http://localhost:8000 \
  -H "Content-Type: application/json" \
  -d '{"key":"mykey","value":"myvalue"}'
```

Retrieve a value:

```bash
curl "http://localhost:8000?key=mykey"
```

## Building manually

```bash
cargo build --target wasm32-wasip2 --release
```

The compiled component is written to `target/wasm32-wasip2/release/http_kv_service.wasm`.
