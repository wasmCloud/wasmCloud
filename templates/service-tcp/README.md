# service-tcp

A wasmCloud v2 service-and-component template written in Rust that demonstrates the **wasmCloud service model** with `wasi:sockets`.

```                                                                       
┌──────────┐   POST /task   ┌──────────┐   TCP :7777   ┌──────────────┐   
│  Browser  │ ────────────► │ http-api │ ────────────► │ service-leet │   
│  (user)   │ ◄──────────── │(component)│ ◄──────────── │  (service)   │  
└──────────┘   "h3110"      └──────────┘   "h3110"     └──────────────┘   
                             stateless                  long-running      
                             per-request                persistent TCP    
                             scales out                 listener          
```             

The template has two parts:

| Part | Type | What it does |
|---|---|---|
| `service-leet/` | wasmCloud **service** workload | TCP server — accepts connections on `127.0.0.1:7777`, reads lines, replies with leet-speak text |
| `http-api/` | wasmCloud **component** workload | HTTP API — `POST /task` forwards text to the TCP service and returns the transformed result |

## Architecture

```
HTTP client
    │  POST /task  {"payload": "Hello World"}
    ▼
http-api (wasi:http/incoming-handler)
    │  TCP connect 127.0.0.1:7777
    │  send "Hello World\n"
    ▼
service-leet (wasi:cli/run)
    │  transform → "H3110 W0r1d"
    │  send "H3110 W0r1d\n"
    ▼
http-api
    │  "H3110 W0r1d"
    ▼
HTTP client
```

Both workloads run in the same wasmCloud host process and communicate over an **in-process loopback network**. Services bind to `0.0.0.0` but the runtime rewrites that to `127.0.0.1` (loopback only — services are never reachable from outside the host).

## wasmCloud v2 service model

wasmCloud supports two kinds of Wasm workloads, and this template uses both:

### Components — stateless and fast

The `http-api/` workload is a standard **HTTP component** that exports `wasi:http/incoming-handler`. The runtime instantiates it on demand for each incoming request, runs it to completion, and tears it down. Because components hold no state between invocations, the host can scale them horizontally — spinning up as many concurrent instances as needed to meet demand, with near-zero overhead per instance.

### Services — long-running and stateful

The `service-leet/` workload is a **service** — a long-running process that exports `wasi:cli/run`. The runtime calls it once on startup and expects it to block indefinitely in a server loop. Services can maintain state across requests — open sockets, connection pools, caches, background tasks — anything that is expensive to create per-request and benefits from being long-lived.

Services bind TCP sockets to loopback (`127.0.0.1`). The runtime enforces this: any `0.0.0.0` bind is silently rewritten to `127.0.0.1`. Components on the same host can connect to those loopback addresses. External callers cannot.

### Why both?

By combining both models, you can keep the request-handling path as lean and scalable as possible while offloading stateful concerns to a dedicated service. Components give you massive, fine-grained scalability for stateless work (HTTP APIs, event processors, data transformations). Services give you a persistent home for connection pools, protocol listeners, caches, and background workers.

See the [wasmCloud components documentation](https://wasmcloud.com/docs/concepts/components).

## Prerequisites

- [Rust](https://rustup.rs/) with the `wasm32-wasip2` target:
  ```bash
  rustup target add wasm32-wasip2
  ```
- [Wasm Shell (`wash`)](https://wasmcloud.com/docs/installation)

## Quick start

```bash
# Start the development server (builds, deploys, watches for changes)
wash dev
```

The HTTP API is available at `http://localhost:8000`.

```bash
# Convert text to leet speak
curl -X POST http://localhost:8000/task \
  -H "Content-Type: application/json" \
  -d '{"payload": "Hello World"}'
# => H3110 W0r1d

# Open the web UI
open http://localhost:8000/
```

## Project structure

This template creates the following structure:

```
service-tcp/
├── .wash/config.yaml          # wash (wasmCloud Shell) project config (build + dev)
├── Cargo.toml                 # Rust workspace root
├── wit/
│   └── world.wit              # WIT world definition
│
├── service-leet/              # TCP leet-speak server
│   ├── src/main.rs            # wasi:cli/run service implementation
│   └── Cargo.toml
│
└── http-api/                  # HTTP front-end
    ├── src/lib.rs             # HTTP handler — POST /task → TCP service → response
    ├── ui.html                # Web UI served at /
    └── Cargo.toml
```

## How it works

The `.wash/config.yaml` file tells `wash dev` how to build and run this project:

- **`build.command`** — Builds the entire workspace for `wasm32-wasip2`
- **`build.component_path`** — Points to the HTTP API component (the primary request handler)
- **`dev.service_file`** — Points to the TCP service, which `wash dev` starts as a long-running background process alongside the HTTP handler

The runtime treats these two workloads differently:

- The **component** (`http-api`) is instantiated per-request. The host can run many instances concurrently with minimal overhead, scaling horizontally to match traffic.
- The **service** (`service-leet`) is started once and runs continuously. It owns the TCP listener and its connection state, providing a stable endpoint that the ephemeral component instances connect to.

## Customizing

### Change what the service computes

Edit `service-leet/src/main.rs` — the `to_leet_speak()` function. The protocol is line-oriented: the component sends a line terminated with `\n`; the service replies with a line. Save the file and `wash dev` will rebuild automatically.

### Add HTTP routes

Edit `http-api/src/lib.rs`. Add match arms to the `main()` function for new paths. The `handle_task()` function encapsulates the TCP client logic; you can extend it or add your own handlers that connect to the service.

### Change the TCP port

The port is `7777`. Update the bind address in `service-leet/src/main.rs` and the connect address in `http-api/src/lib.rs` to the same value.
