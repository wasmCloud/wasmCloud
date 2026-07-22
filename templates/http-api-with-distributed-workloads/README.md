# http-api-with-distributed-workloads

A wasmCloud template demonstrating an HTTP API with distributed workloads via
[`wasmcloud:messaging`][messaging]. An HTTP API receives requests and delegates
processing to background worker components over the messaging interface, with
each worker subscribed to its own subject.

[messaging]: https://github.com/wasmCloud/wasmCloud/tree/main/wit/messaging

| Part | Type | What it does |
|---|---|---|
| `http-api/` | wasmCloud **component** | HTTP API — `POST /task` dispatches work to `tasks.{worker}` via `wasmcloud:messaging/consumer` and returns the reply |
| `task-leet/` | wasmCloud **component** | Worker subscribed to `tasks.leet` — exports `wasmcloud:messaging/handler`, converts payloads to leet speak |
| `task-reverse/` | wasmCloud **component** | Worker subscribed to `tasks.reverse` — exports `wasmcloud:messaging/handler`, reverses the text |

## Architecture

```
┌────────┐      ┌──────────┐      ┌────────────────┐      ┌──────────────┐
│ Client │─────▶│ HTTP API │─────▶│ Message Broker │──┬──▶│ task-leet    │  tasks.leet
│        │◀─────│ (:8000)  │◀─────│ (in-memory or  │  │   └──────────────┘
└────────┘      └──────────┘      │     NATS)      │  │   ┌──────────────┐
                    │             └────────────────┘  └──▶│ task-reverse │  tasks.reverse
              POST /task                                   └──────────────┘
              { worker, payload }
```

1. Client sends a POST request to `/task` with a JSON payload.
2. `http-api` publishes a request to subject `tasks.{worker}` (default: `tasks.leet`).
3. The broker routes the message to the worker subscribed to that subject.
4. The worker processes the payload and publishes the response to the `reply_to` subject.
5. `http-api` returns the transformed response to the HTTP client.

During development with `wash dev`, the wasmCloud runtime routes calls between
components in-process — no NATS server required. In production, the runtime's
built-in messaging plugin connects to NATS automatically when the host starts
with a NATS URL (`wash host --data-nats-url nats://...`). No separate provider
deployment is needed.

## Prerequisites

- [Wasm Shell (`wash`)][wash]
- [Rust toolchain][rust] with the `wasm32-wasip2` target (`rustup target add wasm32-wasip2`)

[wash]: https://wasmcloud.com/docs/installation
[rust]: https://www.rust-lang.org/tools/install

## Quick start

Use `wash new` to scaffold a new project:

```shell
wash new https://github.com/wasmCloud/wasmCloud.git \
  --name http-api-with-distributed-workloads \
  --subfolder templates/http-api-with-distributed-workloads
```

```shell
cd http-api-with-distributed-workloads
```

Start the development server:

```shell
wash dev
```

Then open http://localhost:8000 to use the web UI, or test with curl:

```shell
# Routes to task-leet (tasks.leet)
curl -s -X POST http://localhost:8000/task \
  -H 'Content-Type: application/json' \
  -d '{"worker": "leet", "payload": "Hello World"}'
# => 🤖 H3110 W0r1d

# Routes to task-reverse (tasks.reverse)
curl -s -X POST http://localhost:8000/task \
  -H 'Content-Type: application/json' \
  -d '{"worker": "reverse", "payload": "Hello World"}'
# => 🔁 World Hello
```

The `worker` field selects the subject (`tasks.{worker}`), routing the request
to the worker subscribed to it: `leet` → task-leet, `reverse` → task-reverse.
The request has a 5-second timeout for the worker to respond.

## Project structure

```
.
├── .wash/config.yaml          # wash build & dev configuration (workload + per-component config)
├── Cargo.toml                 # cargo workspace root
│
├── http-api/                  # HTTP front-end component
│   ├── src/lib.rs             # wstd HTTP server + messaging consumer usage
│   ├── ui.html                # branded task-submission UI
│   └── ...
│
├── task-leet/                 # Leet-speak worker (tasks.leet)
│   └── src/lib.rs             # messaging handler + wasi:config + leet logic
│
├── task-reverse/              # Reverse worker (tasks.reverse)
│   └── src/lib.rs             # messaging handler + wasi:config + reverse logic
│
└── wit/world.wit              # shared WIT worlds for the components
```

## Per-component configuration

`.wash/config.yaml` shows how config is layered across a multi-component
workload.

```yaml
dev:
  components:
    - name: task-leet
      file: target/wasm32-wasip2/release/task_leet.wasm
      config:                  # this worker's overrides
        subscriptions: tasks.leet
        leet.mode: aggressive
        leet.prefix: "🤖 "
    - name: task-reverse
      file: target/wasm32-wasip2/release/task_reverse.wasm
      config:
        subscriptions: tasks.reverse
        reverse.mode: words
        reverse.prefix: "🔁 "
```

Two kinds of per-component config are at work:

- **`subscriptions`** is read by the messaging backend (in-memory for
  `wash dev`, or NATS) to decide which subjects each worker receives. This is
  what makes `tasks.leet` route to task-leet and `tasks.reverse` to
  task-reverse rather than both workers competing for every message.
- **`consumer_group`** controls NATS delivery across replicas. When omitted,
  the runtime derives a stable group from the workload namespace, workload
  name, and component name, so one replica handles each message. Set a custom
  non-empty value to share a group explicitly, or set it to `broadcast` when
  every replica must receive every message. Component-local configuration
  overrides the value on the workload's messaging host interface.
- **`leet.*` / `reverse.*`** are read by the worker itself via
  `wasi:config/store` (see `task-leet/src/lib.rs`, `task-reverse/src/lib.rs`).
  `leet.mode` toggles whether `l`/`t` are also substituted; `reverse.mode`
  switches between reversing characters and words; the `*.prefix` is prepended
  to each reply.

## wasmcloud:messaging

This template demonstrates both sides of the `wasmcloud:messaging` interface:

| Interface | Direction | Purpose |
|---|---|---|
| `wasmcloud:messaging/consumer` | import | Send messages (`request`, `publish`) |
| `wasmcloud:messaging/handler` | export | Receive messages (`handle-message`) |

All three components import `consumer` — `http-api` to dispatch tasks via
`request()`, the workers to publish replies via `publish()`. Each worker exports
`handler` and subscribes to its own subject.

## Build Wasm binaries

```bash
wash build
```

Artifacts:
- `target/wasm32-wasip2/release/http_api.wasm`
- `target/wasm32-wasip2/release/task_leet.wasm`
- `target/wasm32-wasip2/release/task_reverse.wasm`

## WIT interfaces

```wit
// wit/world.wit
world http-api {
  import wasmcloud:messaging/consumer@0.2.0;
  // wasi:http/incoming-handler is exported via wstd's #[http_server] macro.
}

world task {
  import wasmcloud:messaging/consumer@0.2.0;
  import wasi:config/store@0.2.0-rc.1;
  export wasmcloud:messaging/handler@0.2.0;
}
```

## Production deployment

All three components run on the same wasmCloud host. Deploy them together using
a `WorkloadDeployment` manifest, giving each worker its own `subscriptions`:

```yaml
apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: http-api-with-distributed-workloads
spec:
  replicas: 1
  template:
    spec:
      hostInterfaces:
        - namespace: wasi
          package: http
          interfaces:
            - incoming-handler
          config:
            host: your-domain.example.com    # HTTP Host header used for routing
      components:
        - name: http-api
          image: <registry>/http_api:latest
        - name: task-leet
          image: <registry>/task_leet:latest
          localResources:
            config:
              subscriptions: tasks.leet      # subjects this worker subscribes to
              leet.mode: aggressive
              leet.prefix: "🤖 "
        - name: task-reverse
          image: <registry>/task_reverse:latest
          localResources:
            config:
              subscriptions: tasks.reverse
              reverse.mode: words
              reverse.prefix: "🔁 "
```

The `hostInterfaces` block declares which built-in capabilities the workload
needs. No separate HTTP server or NATS messaging component is required; both are
provided by the runtime. `subscriptions` is comma-separated NATS subject
patterns; e.g. `tasks.>` matches any subject starting with `tasks.`. 

Replicas join a per-component consumer group by default. 
Add `consumer_group: broadcast` to a component's `localResources.config` when
each replica should receive its own copy (i.e. N components, N copies delivered).

For Kubernetes deployment, see the
[runtime-operator documentation](https://github.com/wasmCloud/wasmCloud/tree/main/runtime-operator).
