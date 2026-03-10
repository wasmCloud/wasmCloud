# couchbase

A WebAssembly HTTP server component that stores and retrieves JSON documents in Couchbase using the `wasmcloud:couchbase/document` interface.

## What it does

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/?key=<key>` | Fetch a document by key |
| `POST` | `/<key>` | Upsert a document (body must be JSON) |
| `DELETE` | `/<key>` | Remove a document |

The component itself has no knowledge of the Couchbase cluster. It speaks only the `wasmcloud:couchbase/document` WIT interface; the host runtime wires that interface to the cluster URL you supply.

## Prerequisites

- Rust with the `wasm32-wasip2` target:
  ```bash
  rustup target add wasm32-wasip2
  ```
- The `wash` CLI (built from this repository)
- Docker (to run a local Couchbase cluster)

## How it works

### 1. The WIT interface

[`wit/world.wit`](wit/world.wit) declares what the component imports and exports:

```wit
world couchbase-example {
  import wasmcloud:couchbase/types@0.1.0-draft;
  import wasmcloud:couchbase/document@0.1.0-draft;

  export wasi:http/incoming-handler@0.2.2;
}
```

`wasmcloud:couchbase/document` provides six functions — `get`, `exists`, `insert`, `upsert`, `replace`, and `remove` — that map directly to Couchbase key-value operations. The component calls these as ordinary Rust functions; the host resolves the calls to the live cluster at runtime.

### 2. The component

[`src/lib.rs`](src/lib.rs) is a standard `wasi:http/incoming-handler` component. On each request it:

1. Parses the HTTP method and path/query to decide which document operation to run.
2. Calls the appropriate `wasmcloud:couchbase/document` function.
3. Serializes the result and writes it to the HTTP response body.

No Couchbase SDK, no connection strings, no credentials — all of that lives in the host.

### 3. The host plugin

The `WasmcloudCouchbase` plugin (in `crates/wash-runtime/src/plugin/wasmcloud_couchbase/`) receives the cluster URL at host startup, keeps a shared `reqwest::Client`, and forwards each WIT call to the Couchbase Management REST API (`/pools/default/buckets/{bucket}/docs/{key}`).

The target bucket is taken from the `bucket` key in the per-interface component configuration (set in `.wash/config.yaml`).

## Running locally

### Step 1 — Start Couchbase with Docker

```bash
docker run -d \
  --name couchbase \
  -p 8091-8097:8091-8097 \
  -p 11210:11210 \
  couchbase:community
```

Wait about 15 seconds for the node to initialise, then run the one-time cluster setup:

```bash
# Initialize the cluster and set admin credentials in one call.
# Note: no -u flag here — the cluster has no credentials yet.
curl -s http://localhost:8091/clusterInit \
  -d clusterName=local \
  -d username=Administrator \
  -d password=password \
  -d port=8091 \
  -d memoryQuota=512 \
  -d services=kv

# Create the demo bucket (100 MB RAM quota).
# Now we authenticate with the credentials set above.
curl -s -u Administrator:password \
  http://localhost:8091/pools/default/buckets \
  -d name=demo \
  -d ramQuotaMB=100 \
  -d bucketType=couchbase
```

Verify the bucket was created (expect `HTTP 200` and `demo` printed):

```bash
curl -o /dev/null -s -w "%{http_code}\n" \
  -u Administrator:password \
  http://localhost:8091/pools/default/buckets/demo
```

Expected output: `200`

### Step 2 — Configure the example

[`.wash/config.yaml`](.wash/config.yaml) already has the cluster URL and bucket name set. If you used different credentials or a different bucket name, edit the relevant lines:

```yaml
dev:
  # URL of the Couchbase cluster management API
  couchbase_url: http://Administrator:password@localhost:8091

  # Per-interface config: which bucket this component targets
  host_interfaces:
    - namespace: wasmcloud
      package: couchbase
      interfaces:
        - types
        - document
      config:
        bucket: demo
```

`host_interfaces` is how the host knows which Couchbase bucket to route calls to for this component. The `bucket` value must match a bucket that exists on the cluster.

### Step 3 — Run with `wash dev`

```bash
wash dev
```

`wash dev` builds the component, starts the embedded HTTP server on `http://localhost:8000`, and hot-reloads on source changes.

## Usage

### Upsert a document

```bash
curl -X POST http://localhost:8000/user-1 \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice", "role": "admin"}'
```

Response:
```json
{"key":"user-1","cas":0}
```

### Fetch a document

```bash
curl "http://localhost:8000/?key=user-1"
```

Response:
```json
{"key":"user-1","content":{"name":"Alice","role":"admin"},"cas":0}
```

### Upsert with an expiry

Documents can be set to expire automatically. Pass `expiry` (seconds) as a query parameter:

```bash
curl -X POST "http://localhost:8000/session-abc?expiry=3600" \
  -H "Content-Type: application/json" \
  -d '{"token": "xyz123"}'
```

### Delete a document

```bash
curl -X DELETE http://localhost:8000/user-1
```

Response:
```
Deleted 'user-1'
```

### Document not found

```bash
curl -v "http://localhost:8000/?key=does-not-exist"
# → HTTP 404
# Document 'does-not-exist' not found
```

## Building manually

```bash
cargo build --target wasm32-wasip2 --release
```

The compiled component is written to `target/wasm32-wasip2/release/couchbase_example.wasm`.

## Notes on CAS

The `cas` field in responses is a best-effort value derived from the Couchbase REST API `rev` field. The REST API does not expose the full binary-protocol CAS on write operations, so `cas` is always `0` after a successful upsert/replace. A future implementation using the Couchbase binary protocol would populate this correctly.

## Stopping

```bash
# Stop and remove the Couchbase container
docker rm -f couchbase
```
