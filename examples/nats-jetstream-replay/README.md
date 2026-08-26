# NATS JetStream order processor

A component that consumes orders from a JetStream stream, accumulates per-order
totals in a JetStream KV bucket, and publishes a processed-order notification —
using `wasmcloud:nats` rather than `wasmcloud:messaging`.

The point of the example is what `wasmcloud:messaging` cannot express: durable
delivery with explicit acknowledgement, redelivery on failure, compare-and-swap
on a KV revision, and publish deduplication. Those are the reasons the
NATS-native interface exists.

## What it demonstrates

| Behaviour | Where |
| --- | --- |
| At-least-once delivery, and how to actually be idempotent | `accumulate` |
| Auto-ack: returning `Ok` acks, `Err` naks and redelivers | `handle_message` |
| Dropping a poison message instead of retrying it forever | the malformed-body branch |
| CAS on a KV revision, with retry on conflict | `accumulate` |
| Redelivery dedupe by stream sequence | `Total.last_sequence` |
| Publish deduplication via `Nats-Msg-Id` | the notification publish |
| Typed errors instead of string matching | `describe` |

## Prerequisites

A NATS server with JetStream, plus the stream and bucket this component expects.
Neither is created by the component: stream and bucket lifecycle is deliberately
outside `wasmcloud:nats`, so a workload cannot provision storage it was not
granted.

```bash
nats-server -js &

nats stream add ORDERS \
  --subjects 'orders.received' \
  --storage file --retention limits --discard old \
  --max-msgs=-1 --max-bytes=-1 --max-age=24h \
  --dupe-window=2m --replicas 1 --defaults

nats stream add PROCESSED \
  --subjects 'orders.processed' \
  --storage file --dupe-window=5m --replicas 1 --defaults

nats kv add order-totals --history 5
```

The `--dupe-window` on `PROCESSED` is what makes the `Nats-Msg-Id` header do
anything. Without it, a redelivered order publishes a second notification.

## Build

```bash
wash build
```

## Deploy

Where a binding points, as whom, and what it may reach are the *host's* to
declare — a workload asks for a binding and receives what the operator granted
it. On a cluster that lives in the chart
(`runtime.hostGroups[].wasmcloudNats`), which renders into the host's config
file:

```yaml
host:
  wasmcloudNats:
    config:
      servers: nats://nats.default.svc:4222
      # Deny-by-default: without these the workload reaches nothing. A
      # subscription's filter subject is checked against this too, so
      # `orders.received` has to be listed even though the grant on the
      # ORDERS stream is what selects the stream.
      subject-allow: orders.processed,orders.received
      stream-allow: ORDERS,PROCESSED
      bucket-allow: order-totals
    # Credentials never appear in a manifest, and never on a command line.
    secretFrom:
      - nats-credentials
```

This component imports `wasmcloud:nats` plainly, so it gets the *unnamed*
binding — the block above. A workload that wants two bindings labels its
imports (`(implements orders)`) and the host declares each under
`wasmcloudNats.bindings.<name>`; label routing is served by the async
`@0.2.0` package only.

The manifest then says only what it wants delivered:

```yaml
apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: nats-jetstream-replay
spec:
  replicas: 1
  components:
    - name: processor
      image: file://./target/wasm32-wasip2/release/nats_jetstream_replay.wasm
  hostInterfaces:
    - namespace: wasmcloud
      package: nats
      version: "0.1.0"
      interfaces: [types, jetstream, kv, jetstream-handler]
      config:
        # STREAM:filter[:policy[:queue]]
        subscriptions: ORDERS:orders.received:all
        ack-mode: auto
        max-in-flight: "32"
```

`subject-allow`, `stream-allow`, and `bucket-allow` are the capability boundary.
They are separate on purpose: permission to publish to `orders.processed` does
not carry permission to read or delete the `ORDERS` stream. A `wash host`
refuses a manifest that sets any of them, or one that names a binding it does
not serve — a workload can ask for a capability, never widen one.

Credentials never appear in `config`. The host merges
`config` → `configFrom` → `secretFrom` (later wins) before the plugin sees them,
so a creds file, JWT + nkey seed, username/password, or token arrives already
resolved. An nkey seed is signed host-side and never crosses into the sandbox.

### Under `wash dev`

`wash dev` leaves the manifest free to describe its own binding, so a project
stays runnable on its own: put the same keys — `servers` and the three grants —
straight in the interface's `config`, or declare them once under
`dev.wasmcloud_nats` in `.wash/config.yaml`. `dev.wasmcloud_nats_url` (falling
back to `dev.data_nats_url`) is the address a binding that names no `servers`
falls back to.

## Try it

```bash
nats pub orders.received "order-1:100"
nats pub orders.received "order-1:50"

nats kv get order-totals order-1     # -> 150@2   (total@last-applied-sequence)
nats sub orders.processed            # -> order-1:100, then order-1:150
```

Replay is what the stream buys you. Deleting the KV bucket and replaying the
stream from sequence 1 rebuilds every total, because the orders are still there:

```bash
nats kv del order-totals --force
nats kv add order-totals --history 5
nats consumer add ORDERS replay --deliver all --ack explicit --defaults
```

A malformed body is acked and dropped rather than redelivered forever. Under
`ack-mode: auto` the host owns the acknowledgement, so returning `Err` would nak
and retry something that can never succeed; `term()` needs `ack-mode: manual`.

```bash
nats pub orders.received "not-an-order"
# -> "dropping malformed order at sequence N" in the host log, then acked
```

## Notes

- `ack-mode: auto` means the host acks on `Ok` and naks on `Err` or trap. Set
  `manual` to take over, and call `handle.ack()` / `nak()` / `term()` yourself.
- `max-in-flight` bounds concurrent handler invocations per consumer. Without a
  bound, a backlog spike fans out into the component pool all at once.
- The component is per-request. It holds no consumer and no stream, so it scales
  down to nothing between bursts — the host owns the subscription.
