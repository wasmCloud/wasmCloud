# HTTP + Key-Value Service in Rust

This project template is a WebAssembly component built with [Rust][rust] that stores and retrieves key-value pairs over HTTP, backed by [`wasi:keyvalue`][wasi-kv].

The component speaks only the `wasi:keyvalue/store` interface. The host runtime selects the underlying storage backend (in-memory, filesystem, NATS, Redis, and others) based on `.wash/config.yaml`, so the same component code runs against any supported backend without modification.

[rust]: https://www.rust-lang.org/
[wasi-kv]: https://github.com/WebAssembly/wasi-keyvalue

## Prerequisites

- [Wasm Shell (`wash`)][wash]
- [Rust toolchain][rust-install]
- The `wasm32-wasip2` Rust target: `rustup target add wasm32-wasip2`

[wash]: https://wasmcloud.com/docs/installation
[rust-install]: https://www.rust-lang.org/tools/install

## Local development

Use `wash new` to scaffold a new wasmCloud component project:

```shell
wash new https://github.com/wasmCloud/wasmCloud.git --name http-kv-service --subfolder templates/http-kv-service
```

```shell
cd http-kv-service
```

To build this project and run in a hot-reloading development loop, run `wash dev` from this directory:

```shell
wash dev
```

## Endpoints

| Endpoint | Method | Description |
| -------- | ------ | ----------- |
| `/` | POST | Stores a key-value pair from a JSON body `{"key":"...","value":"..."}` |
| `/?key=<key>` | GET | Returns the value stored at `<key>`, or `404` if the key does not exist |

## Send requests to the running component

```shell
# Store a value
curl -X POST http://localhost:8000 \
  -H "Content-Type: application/json" \
  -d '{"key":"mykey","value":"myvalue"}'

# Retrieve a value
curl "http://localhost:8000?key=mykey"
```

## Choosing a backend

The `BACKEND` constant in [src/lib.rs](src/lib.rs) controls which bucket name is passed to `open()`. The host runtime selects the actual storage backend based on [.wash/config.yaml](.wash/config.yaml).

Set `BACKEND` to one of the following values and uncomment the matching section in `.wash/config.yaml`:

| `BACKEND`      | Description                                | Required config key            | Example value                    |
|----------------|--------------------------------------------|--------------------------------|----------------------------------|
| `"in_memory"`  | Ephemeral in-process store (default)       | *(none)*                       |                                  |
| `"filesystem"` | Persists data to a local directory         | `wasi_keyvalue_path`           | `/tmp/keyvalue-store`            |
| `"nats"`       | Uses [NATS][nats] JetStream as the store   | `wasi_keyvalue_nats_url`       | `nats://127.0.0.1:4222`          |
| `"redis"`      | Uses a [Redis][redis] server as the store  | `wasi_keyvalue_redis_url`      | `redis://127.0.0.1:6379`         |

[nats]: https://nats.io/
[redis]: https://redis.io/

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

```shell
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

```shell
docker run --name redis -p 6379:6379 redis:latest
```

## Build Wasm binary

```shell
wash build
```

## WIT Interfaces

This component uses the following [WIT interfaces](https://component-model.bytecodealliance.org/design/wit.html):

```wit
world http-kv-service {
  import wasi:keyvalue/store@0.2.0-draft;

  export wasi:http/incoming-handler@0.2.2;
}
```
