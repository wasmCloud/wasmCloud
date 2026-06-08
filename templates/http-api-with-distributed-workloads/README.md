# HTTP API with Distributed Workloads

A wasmCloud template demonstrating distributed workloads using messaging. An HTTP API receives requests and delegates processing to background workers via a message broker.

## Architecture

```
┌────────┐      ┌──────────┐      ┌─────────────────┐      ┌─────────────┐
│ Client │─────▶│ HTTP API │─────▶│ Message Broker  │─────▶│ Task Worker │
│        │◀─────│ (:8000)  │◀─────│ (NATS)          │◀─────│ (task-leet) │
└────────┘      └──────────┘      └─────────────────┘      └─────────────┘
                    │                                            │
              POST /task                                   Transforms text
              { worker, payload }                          to leet speak
```

### Components

- **http-api**: HTTP server exposing the `/task` endpoint
- **task-leet**: Message handler that processes tasks (converts text to leet speak)

### How It Works

1. Client sends a POST request to `/task` with a JSON payload
2. HTTP API publishes a request to subject `tasks.{worker}` (default: `tasks.default`)
3. Task worker receives the message and processes the payload
4. Worker publishes the response back via the `reply_to` subject
5. HTTP API returns the transformed response to the client

The request has a 5-second timeout for the worker to respond.

## The /task Endpoint

### Request

```
POST /task
Content-Type: application/json

{
  "worker": "default",  // optional, defaults to "default"
  "payload": "Hello World"
}
```

The `worker` field determines the messaging subject (`tasks.{worker}`), allowing you to route to different workers.

### Response

The transformed payload from the worker:

```
🤖 H3110 W0r1d
```

The `🤖 ` prefix and aggressive leet substitution come from the worker's
per-component config — see [Per-component configuration](#per-component-configuration).

### Example

```bash
wash dev
```

Then open [http://localhost:8000/](http://localhost:8000/).

Using the `/task` endpoint directly:

```bash
curl -X POST http://localhost:8000/task \
  -H "Content-Type: application/json" \
  -d '{"payload": "Hello World", "worker": "leet"}'
```

## Per-component configuration

`.wash/config.yaml` shows how config is layered across a multi-component
workload. The `workload:` block is the shared base.
Each `dev.components` entry can override it on a per-key basis, mirroring the
per-component `localResources` of a Kubernetes CRD `WorkloadDeployment`:

```yaml
dev:
  components:
    - name: task-leet
      file: target/wasm32-wasip2/release/task_leet.wasm
      config:                  # this worker's overrides
        leet.mode: aggressive
        leet.prefix: "🤖 "

workload:
  config:                      # shared defaults for every component
    leet.mode: basic
    leet.prefix: ""
```

The `task-leet` worker reads these via `wasi:config/store` (see
`task-leet/src/lib.rs`): `leet.mode` toggles whether `l`/`t` are also
substituted, and `leet.prefix` is prepended to each reply. Because the
component's `config:` overrides the workload defaults, it runs in `aggressive`
mode with the `🤖 ` prefix. Drop the per-component block and it falls back to
the workload defaults (`basic`, no prefix → `H3llo W0rld`).
