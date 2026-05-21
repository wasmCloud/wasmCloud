# patch-stream

Proof-of-concept that three wasmCloud components rendezvous over a
host-provided pubsub broker so that an HTTP request can drive a
long-lived WebSocket session, with payloads flowing over a wasip3
cross-component `stream<u8>` along the way.

- **page-agent** exports `page-generation.generate-page(prompt) ->
  stream<u8>`. It first tries an OpenAI-compatible streaming
  chat-completions call when `OPENAI_API_KEY` or
  `PAGE_AGENT_OPENAI_API_KEY` is set; otherwise it falls back to a
  deterministic local 14-edit demo stream paced at 500 ms per write,
  with `[t+NNNms]` prefixes so cadence is visible end-to-end.
- **commander** exports `wasi:http/handler@0.3`. On `GET
  /?prompt=...`, it calls `page-generation.generate-page(prompt)` and
  hands the returned stream to MetaJson's `sink.send-stream`. The
  HTTP response itself is just an empty 200/502 ack — commander's
  job is to fan-in the trigger, not to carry payload.
- **meta-json** exports both `wasmcloud:patch-stream/sink` and
  `wasmcloud:websocket/handler@0.1`. The two run on completely
  separate requests:
  - WS clients open `ws://localhost:8000/`. meta-json's WS handler
    `register()`s with the broker, then loops `broker.wait-message()`
    and emits each received line as a `Frame::Text`.
  - When commander invokes `sink.send-stream(stream)`, meta-json
    drains the bytes, splits on `\n`, and `broker.publish-message()`s
    each line. The broker fans it out to every connected WS client.
- **wasmcloud:patch-stream/broker** is a tiny per-workload pubsub
  host plugin in `wash-runtime` (not a component). It owns a
  `tokio::sync::broadcast` channel per workload, hands out
  subscriber IDs to `register()`, and routes `publish-message()`
  to every registered subscriber's `wait-message()`. This is what
  bridges the WS connect (request A) and the HTTP trigger
  (request B) — neither component sees the other directly.

```
                      ┌─────────────────────────────────┐
   HTTP GET           │ commander                       │  generate-page(prompt)   ┌──────────────────┐
   /?prompt=...  ───▶ │ exports wasi:http/handler@0.3   │ ────────────────────▶    │ page-agent       │
   (one-shot ack)     │ imports page-generation, sink   │                          │ exports          │
                      └─────────────────────────────────┘                          │ page-generation  │
                              │                                                    │                  │
                              │ sink.send-stream(stream<u8>) ◀────── stream<u8> ───│ AI or demo       │
                              ▼                                       NDJSON       │ fallback         │
                      ┌─────────────────────────────────┐                          └──────────────────┘
                      │ meta-json                       │
                      │ exports sink                    │
                      │ imports broker                  │
                      │   sink.send-stream:             │
                      │     drain bytes → publish lines │
                      └─────────────────────────────────┘
                              │
                              │ broker.publish-message(line)
                              ▼
                      ┌─────────────────────────────────┐
                      │ wasmcloud:patch-stream/broker   │  ← host plugin
                      │ (tokio::sync::broadcast per     │     in wash-runtime
                      │  workload)                      │
                      └─────────────────────────────────┘
                              │
                              │ broker.wait-message() resolves
                              ▼
                      ┌─────────────────────────────────┐    WS text frame      ┌─────────┐
                      │ meta-json                       │ ───────────────────▶  │ websocat│
                      │ exports websocket/handler@0.1   │ ◀── WS upgrade ─────  │ /browser│
                      │   handle: register → loop       │                       │         │
                      │     wait_message → Frame::Text  │                       │         │
                      └─────────────────────────────────┘                       └─────────┘
```

Same `:8000` listener for both the HTTP trigger and the WS upgrade.
The host's HTTP-vs-WS branch happens in
`host/http.rs::is_websocket_upgrade`. Same workload for all three
components — they rendezvous through the broker, not through
component-to-component imports.

## Run

You need the locally-built wash at `../../target/debug/wash` (built
with `cargo +nightly build -p wash --features wasip3` from the
wasmCloud repo root). All three components need a recent nightly
rustc for wit-bindgen's wasi:http@0.3 custom sections.

```sh
../../target/debug/wash dev
```

The flow needs two clients: a WS subscriber and an HTTP publisher.

**Terminal A — connect a WS client first** (it'll just sit there
until commander is poked):

```sh
websocat ws://localhost:8000/
```

**Terminal B — fire the HTTP trigger with a prompt:**

```sh
curl -i 'http://localhost:8000/?prompt=Make%20a%20landing%20page%20for%20streaming%20agents'
```

curl prints an empty 200 response. **Terminal A** then renders the
patches as they're produced. Without an OpenAI key the deterministic
fallback emits 14 lines, one per ~500 ms:

```text
[t+   0ms] {"op":"add","path":"/title","value":"\"Untitled\""}
[t+ 500ms] {"op":"add","path":"/version","value":"0"}
[t+1000ms] {"op":"add","path":"/items","value":"[]"}
[t+1500ms] {"op":"add","path":"/prompt","value":"\"Make a landing page for streaming agents\""}
[t+2000ms] {"op":"replace","path":"/title","value":"\"AI-assisted streaming demo\""}
... (more lines, ~500 ms apart) ...
```

With an API key set in the wash dev environment, page-agent streams
OpenAI's response token-by-token; the same lines flow to every
attached WS client. Open multiple `websocat` sessions before
triggering curl to see broker fan-out in action — every subscriber
receives the same sequence.

## WebSocket egress (meta-json)

meta-json's WS handler does **not** call page-agent. The whole point
of the broker indirection is that the WS handler is decoupled from
whoever happens to be generating patches. On each connection it:

1. Calls `broker.register()` to get a client-id (sync host call;
   returns a `u64`).
2. Spawns a `wit_bindgen::spawn` task that loops
   `broker.wait-message(client-id).await`. Each non-`None` return is
   a published line; the task writes it as `Frame::Text(line)` into
   meta-json's locally-created `outgoing` stream.
3. Returns the reader end of that stream; the host pipes it into the
   WS write half.

When the broker subscription closes (workload unbind, or explicit
`unregister`) or the WS client disconnects, the spawned loop exits,
`outgoing_tx` drops, the host's stream consumer sees end-of-stream,
and tungstenite sends a normal close (code 1000).

The publishing side runs on a different request entirely: commander
takes an HTTP request, drives page-agent, hands the resulting
`stream<u8>` to meta-json's `sink.send-stream`. meta-json drains
that stream byte-by-byte, splits on `\n`, calls
`broker.publish-message(line).await` per line. The broker fans
out to every subscriber.

```
                              wash dev :8000

   request A (any time)                  request B (later, with prompt)

   ws upgrade ───┐                       http get ───┐
                 ▼                                   ▼
   meta-json.handle(req, incoming)       commander.handle(req)
       broker.register() → cid                page-generation.generate-page(prompt)
       loop:                                  ↓ stream<u8>
         broker.wait-message(cid)          sink.send-stream(stream)
           ▲                                 ↓
           │                                meta-json.send-stream(s)
           │                                  drain s; per line:
           │      ┌──── broker ────┐            broker.publish-message(line)
           └──────│  broadcast::Tx │ ◀─────────────┘
                  └────────────────┘
```

### WS-handling host code lives in

- WIT: [`wit/deps/wasmcloud-websocket-0.1.0/package.wit`](wit/deps/wasmcloud-websocket-0.1.0/package.wit)
  (vendored from `crates/wash-runtime/wit/deps/`).
- Bridge: [`crates/wash-runtime/src/host/websocket.rs`](../../crates/wash-runtime/src/host/websocket.rs)
  — SHA1+base64 handshake, hyper `OnUpgrade` capture, tungstenite
  wrapping, `WsReadProducer` / `WsWriteConsumer` implementing
  wasmtime's `StreamProducer` / `StreamConsumer` for typed Frames.
- Dispatch branch: [`crates/wash-runtime/src/host/http.rs`](../../crates/wash-runtime/src/host/http.rs)
  (`is_websocket_upgrade` + the WS arm in `invoke_component_handler`,
  which calls `pre_instantiate_linked_components_for_component`
  before invoking the WS handler so peer components stay reachable).
- Workload binding: [`crates/wash-runtime/src/engine/workload.rs`](../../crates/wash-runtime/src/engine/workload.rs)
  — `resolve` registers WS-exporting components with the HTTP
  server alongside any wasi:http exporter, so the WS handler
  actually receives incoming upgrade requests on the dev listener.
- Broker plugin: [`crates/wash-runtime/src/plugin/wasmcloud_stream_broker.rs`](../../crates/wash-runtime/src/plugin/wasmcloud_stream_broker.rs)
  — per-workload `tokio::sync::broadcast` channel + client-id
  table. Async `wait-message` / `publish-message` are real awaits
  (no `blocking_*` — that would panic on the tokio worker driving
  the store).

### Handshake-only smoke test

Useful when you don't have a WS-capable client and just want to
verify the host's `Sec-WebSocket-Accept` (SHA1 + base64) is right:

```sh
curl -sv --http1.1 \
  -H 'Connection: Upgrade' \
  -H 'Upgrade: websocket' \
  -H 'Sec-WebSocket-Version: 13' \
  -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
  http://localhost:8000/ 2>&1 | head -20
```

The `Sec-WebSocket-Key` above is the canonical RFC 6455 test
vector; the expected response includes
`sec-websocket-accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`. After the
101, curl can't read frames so it bails with "empty reply" — that's
expected; use `websocat` for the full demo.

### What's exercised by the WS path on top of the HTTP path

- **Host detects WS upgrades from HTTP.** `host/http.rs::is_websocket_upgrade`
  checks `Upgrade`, `Connection`, and `Sec-WebSocket-Version: 13`;
  `engine::targets_websocket` checks the component for a
  `wasmcloud:websocket/*@0.1` export. Both true → request branches
  into `host/websocket.rs` instead of the wasi:http handler.
- **101 handshake is hyper-native.** The host computes
  `Sec-WebSocket-Accept` (SHA1 of the client key + the RFC 6455
  magic GUID, base64-encoded), captures hyper's `OnUpgrade` future
  *before* returning the 101, then awaits the upgrade in a detached
  task to get the raw `TokioIo<Upgraded>`. tokio-tungstenite's
  `WebSocketStream::from_raw_socket(.., Role::Server, None)` takes
  over from there — no second handshake.
- **Typed frame bridging.** WS messages are bridged to/from the
  component as `stream<frame>` (a variant of text/binary/close)
  using wasmtime's `StreamProducer` / `StreamConsumer` traits.
  `WsReadProducer` pulls `Message`s from the tungstenite read half
  and yields `Frame`s into the guest's `incoming` stream;
  `WsWriteConsumer` pulls `Frame`s out of the guest's returned
  outgoing stream and writes `Message`s back. This sidesteps the
  intra-component `host_copy` guard that blocks `stream<record>` on
  the cross-component bridge — wasmtime's host-IO path doesn't go
  through that guard.
- **Forwarder task drives the flush.** `WsWriteConsumer` doesn't
  call `poll_flush` directly. Instead it pushes `Message`s into a
  bounded `tokio::sync::mpsc` channel; a separate
  `ws_write_forwarder` task owns the tungstenite write half and
  loops `ws.send(msg).await`, which is `feed + flush` end-to-end.
  Doing it this way is what makes frames actually appear at the
  client at the producer's cadence rather than buffering in
  tungstenite's RAM until the connection closes.
- **The broker decouples the trigger from the subscriber.** The WS
  handler imports `broker`, not `page-generation`. So the WS
  session can outlive any single commander invocation, and a single
  commander invocation can fan out to N concurrent WS sessions.

## What's actually being exercised

- **Cross-component p3 stream values flow through wash's dynamic
  linker.** The commander's `page_generation::generate_page(prompt)` call invokes the
  PageAgent's exported function via wash's
  `linker_instance.func_new_concurrent` bridge; the returned
  `StreamReader<u8>` handle crosses through wash's value
  lift/lower into the commander.
- **PageAgent background tasks are pumped on the same concurrent
  runtime.** The PageAgent's `wit_bindgen::spawn(async move { writer
  .write_all(...) })` writer task continues running after
  `generate-page` returns its stream handle, because the commander's
  invocation went through `Store::run_concurrent` (set up by
  `host/http_p3.rs`) and the cross-component bridge uses
  `call_concurrent` rather than `call_async`.
- **Zero-copy stream forwarding.** `page_generation::generate_page`
  returns `stream<u8>`, so the commander can hand the PageAgent
  stream directly to MetaJson's sink without parsing or re-encoding
  each patch line.
- **The PageAgent paces and timestamps writes.** Between writes the
  PageAgent awaits `wasi:clocks/monotonic-clock::wait-for(500ms)` —
  that's an `async func` import on a wasip3 host interface, and the
  await actually suspends the writer task without blocking the
  commander's HTTP handler. The `[t+NNNms]` prefix is captured at
  the moment of `write_all`, so the response visibly records the
  PageAgent's rhythm.
- **End-to-end chunked egress.** The response is sent as
  `Transfer-Encoding: chunked`; each `writer.write_all(line)` in
  the PageAgent becomes one HTTP chunk on the wire, flushed by
  hyper as soon as the wasi:http pipe consumer hands it over.
  No collect, no Content-Length, no buffering between guest and
  client — `curl -N` shows lines arrive at the PageAgent's cadence.

## Why the timestamps are stamped in the PageAgent, not the commander

A natural alternative would be: read each patch on the commander
side with `page_rx.next().await`, stamp it with the commander's
clock at the moment it arrives, then push the timestamped line into
a *new* `stream<u8>` and hand that as the response body. The reading
side is fine — `.next()` / `.read(buf)` / `.collect()` all work on
the cross-component reader. The blocker is the *writer* side.

In our commander's WIT world there's only one imported `stream<u8>`
type that wit-bindgen actually emits canonical-ABI builtins for: the
PageAgent result stream. (Inspect the commander wasm: there's
`[stream-new-0]generate-page` under
`wasmcloud:patch-stream/page-generation@0.1.0` and the boilerplate
`[stream-*-unit]`, but nothing like `[stream-new-0][static]request.new`
or `response.new` — wit-bindgen only generates a vtable for stream
types the guest itself constructs.) So `bindings::wit_stream::new::<u8>()`
in the commander routes through the PageAgent stream's slot, not a
wasi:http response-body slot. Handing that `body_rx` to
`Response::new` produces a slot-mismatch and the writer traps with

```
write pointer out of bounds
```

at the first `body_tx.write_all` — the writer's lowered length is
computed against the wrong type entry. Same family of bug as the
cross-component `Instance::copy` issue documented below, just
surfaced via a different lookup.

Pushing the timestamping into the PageAgent dodges this entirely:
the PageAgent already holds a writer for the page stream (it
created the stream with `wit_stream::new::<u8>()` against its
own export's vtable) and has `wasi:clocks/monotonic-clock@0.3`
imported, so stamping each line at write time costs nothing extra.

## Why `stream<u8>` instead of `stream<patch>`

This was the design we wanted; it's blocked by an upstream wasmtime
bug that only surfaces under dynamic linking (i.e. wash's runtime).

The lift/lower of the stream return value across wash's bridge
works fine — verified by trace: `generate-page(prompt)` returns
`Stream(StreamAny { id: TransmitHandle(N), ty:
Guest(StreamType(TypeStreamIndex(M))) })`, wash identity-passes it,
and the commander receives a stream handle without error. Likewise
the per-stream canonical-ABI builtins (`[stream-new-0]generate-page`,
`[stream-read-0]generate-page`, …) are wasmtime-compiled trampolines,
not `Linker` imports, so wash never needs to register them.

The break is in wasmtime's stream-data copy path
([`futures_and_streams.rs::Instance::copy`][copy]):

```rust
let (component, mut store) = self.component_and_store_mut(store.0);
let types = component.types();                    // ← reader's ComponentTypes
let write_payload_ty = write_ty.payload(types);   // ← writer's TypeStreamTableIndex
let read_payload_ty  = read_ty.payload(types);    // ← reader's TypeStreamTableIndex
```

`self` is the **reader's** `Instance`. `write_ty` is the **writer's**
`TypeStreamTableIndex`, stashed in `WriteState::GuestReady` when the
PageAgent's `guest_write` fired. It's then resolved against the
**reader's** `ComponentTypes` table — which only happens to be
meaningful when both ends live in the same composed component
graph (one shared `ComponentTypes`).

Under wash's dynamic linking, PageAgent and commander are **separate
`Component`s with separate `ComponentTypes`**. Looking up the
PageAgent's slot index in the commander's type table returns whatever
the commander happens to have at that slot — usually the body
stream's `stream<u8>`. `copy()` then lifts each item as `u8` and
tries to store it as the commander's expected payload, producing:

```
type mismatch: expected record, found u8     # stream<patch>
type mismatch: expected string, found u8     # stream<string>
```

Declaring the patches stream as `stream<u8>` works not because of
any "fallback" but because both sides' slot-0 stream types happen
to coincide on `u8`, so the cross-instance index lookup returns the
correct payload type by accident. NDJSON encoding makes that
practical: each patch is one JSON line.

The proper fix lives upstream in wasmtime:
- `Instance::copy` (and sibling functions that do the same lookup)
  needs the **writer's** component types for `write_ty.payload(...)`
  and the **reader's** for `read_ty.payload(...)`. The signature
  already carries `write_caller_instance: RuntimeComponentInstanceIndex`
  — wasmtime would need to resolve that to the writer's `Component`
  rather than assuming `self.component()` covers both ends.
- Or, ship a `Linker`-side stream-builtin registration API so hosts
  can route reads/writes through a custom path that knows both
  components' type tables. (Slower; correct; preserves dynamic
  linking.)

Component composition (`wasm-tools compose`) sidesteps the bug by
collapsing everything into one component, but at the cost of giving
up multi-component runtime linking.

[copy]: ../../../wasmtime/crates/wasmtime/src/runtime/component/concurrent/futures_and_streams.rs

## wash-runtime patches required to reach this point

This example would not run on stock wash; it requires the following
changes in `crates/wash-runtime/`:

1. **[`engine/value.rs`](../../crates/wash-runtime/src/engine/value.rs)** —
   `lift`/`lower` for `Val::Stream`, `Val::Future`,
   `Val::ErrorContext` now return identity instead of bailing with
   `"async not supported"`. Single workload `Store` ⇒ the handle is
   valid on both sides.

2. **[`engine/workload.rs`](../../crates/wash-runtime/src/engine/workload.rs)** —
   `resolve_component_imports` registers cross-component imports via
   `linker_instance.func_new_concurrent` (was `func_new_async`) so
   the PageAgent's `wit_bindgen::spawn` writer task gets polled on
   the same concurrent runtime as the commander's `Store::run_concurrent`.
   The new `ResolvedWorkload::pre_instantiate_exporters` populates
   a per-store cache of exporter `Instance`s, since
   `pre.instantiate_async` cannot run inside the `accessor.with`
   sync closure that `func_new_concurrent` provides. `resolve` also
   registers WS-handler-exporting components with the HTTP server
   so they receive incoming `Upgrade: websocket` requests.

3. **[`host/http.rs`](../../crates/wash-runtime/src/host/http.rs)** —
   `invoke_component_handler` calls
   `workload_handle.pre_instantiate_exporters(&mut store, component_id)`
   right after `new_store` and before dispatching to the p2 / p3
   HTTP handler or the WS handler. New `is_websocket_upgrade` helper
   and a WS arm that runs *before* the p3 HTTP arm, so WS upgrade
   requests skip the wasi:http path entirely.

4. **[`host/http_p3.rs`](../../crates/wash-runtime/src/host/http_p3.rs)** —
   Stream the wasi:http response body straight through to hyper
   instead of `body.collect().await`-ing it. The handler future is
   spawned onto tokio; inside its `Store::run_concurrent`, we hand
   hyper a `StreamingBody` wrapper via a `oneshot` and then await a
   completion signal from the wrapper's `Drop` / final
   `poll_frame -> None`. That keeps the concurrent runtime alive
   for the body's lifetime so the wasi-http pipe consumer keeps
   pumping chunks into the body's mpsc. Hyper sees an unbounded
   `size_hint` and picks `Transfer-Encoding: chunked` on its own.

5. **[`host/websocket.rs`](../../crates/wash-runtime/src/host/websocket.rs)** —
   New module. Validates the WS handshake, computes
   `Sec-WebSocket-Accept`, captures hyper's `OnUpgrade`, wraps the
   post-upgrade socket in `tokio_tungstenite::WebSocketStream`, and
   bridges its read/write halves to a typed `stream<frame>` via
   wasmtime's `StreamReader::new` / `.pipe`. A bounded mpsc +
   detached forwarder task owns the WS write half so `send().await`
   drives `feed + flush` end-to-end, which is what makes frames
   actually leave the host at the producer's cadence.

6. **[`engine/mod.rs`](../../crates/wash-runtime/src/engine/mod.rs)** —
   New `targets_websocket(&Component)` detector that recognizes any
   `wasmcloud:websocket/*@0.1` export. Used by the dispatch branch
   above and by `resolve` to decide what to bind to the HTTP server.

7. **[`plugin/wasmcloud_stream_broker.rs`](../../crates/wash-runtime/src/plugin/wasmcloud_stream_broker.rs)** —
   New host plugin. Implements `wasmcloud:patch-stream/broker@0.1`
   with a per-workload `tokio::sync::broadcast` channel plus a
   client-id table. `register` / `unregister` are sync in the WIT;
   `wait-message` / `publish-message` are `async func` and live on
   the `HostWithStore` trait. Both async impls extract a cloned
   `Arc<RwLock<...>>` synchronously under `accessor.with(...)` and
   then `.await` the lock outside the store borrow — `blocking_*`
   panics because we're already on a tokio worker driving the
   store's concurrent runtime.

8. **[`wit/world.wit`](../../crates/wash-runtime/wit/world.wit)** +
   **[`wit/deps/wasmcloud-websocket-0.1.0/package.wit`](../../crates/wash-runtime/wit/deps/wasmcloud-websocket-0.1.0/package.wit)** —
   New `world websocket { export wasmcloud:websocket/handler@0.1.0; }`
   plus the package definition for the `handler` and `types`
   interfaces (`frame` variant, `upgrade-request` record,
   `handle: async func(req, incoming) -> result<stream<frame>, string>`).

## Layout

```
examples/patch-stream/
├── Cargo.toml                # workspace: [page-agent, commander, meta-json]
├── .wash/config.yaml         # wash dev: commander main; page-agent + meta-json peers
├── wit/
│   ├── world.wit             # page-generation + sink + broker + three worlds
│   └── deps/                 # wasi:http@0.3 + clocks@0.3 + wasmcloud:websocket@0.1 + ...
├── page-agent/               # exports page-generation.generate-page;
│                             # imports clocks/http/env. Tries OpenAI streaming
│                             # chat-completions; falls back to a 14-edit
│                             # timestamped NDJSON demo paced at 500ms/line.
├── commander/                # exports wasi:http/handler@0.3; imports
│                             # page-generation + sink. On GET /?prompt=..., calls
│                             # page-agent then hands the stream to meta-json's sink.
└── meta-json/                # exports wasmcloud:patch-stream/sink AND
                              # wasmcloud:websocket/handler@0.1; imports broker.
                              # WS path: register → loop wait_message → Frame::Text.
                              # sink path: drain stream<u8> → publish_message per line.
```

`.wash/config.yaml` sets `dev.wasip3: true`, has commander as the
build target, and lists `page-agent` + `meta-json` under
`dev.components` so wash dev loads them as peers of commander. The
broker isn't listed because it's a host plugin (lives in
wash-runtime), not a component.

## Open follow-ups

- **File the wasmtime bug.** `Instance::copy` resolves `write_ty`
  (the writer's `TypeStreamTableIndex`) against `self.component()`'s
  `ComponentTypes` (the reader's), which is only correct under
  composition. Under dynamic linking the lookup returns the wrong
  payload type and lowering trips the `type mismatch` error. Until
  that's fixed, any wash workload that wants cross-component
  streams is constrained to `stream<T>` payloads where every stream
  type in both components has matching `T` at matching
  `TypeStreamTableIndex` slots. NDJSON `stream<u8>` is the easiest
  way to satisfy that.
- **Expose multiple `wit_stream` vtables in wit-bindgen-generated
  bindings, or let the guest pick one explicitly.** Today, if a
  commander's world declares more than one `stream<T>` for the same
  `T`, `wit_stream::new::<T>()` can route through the wrong slot
  (see "Why the timestamps are stamped in the PageAgent"). The
  workaround is to do the work in a component that owns the right
  vtable; a cleaner fix is upstream in wit-bindgen.
