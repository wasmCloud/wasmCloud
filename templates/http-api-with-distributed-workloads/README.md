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
H3110 W0r1d
```

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
