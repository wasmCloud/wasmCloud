# patch-stream

Proof-of-concept that wasmCloud components can communicate over a
wasip3 cross-component `stream` and surface the result either as a
chunked HTTP body **or** as a stream of WebSocket text frames.

A producer component emits JSON Patch records (RFC 6902, NDJSON-
encoded) into a `stream<u8>`. Two front-end components consume that
stream:

- **patch-consumer** exports `wasi:http/handler@0.3` and pipes the
  stream straight through as the body of a `GET /` HTTP response
  (chunked, no copying).
- **meta-json** exports `wasmcloud:websocket/handler@0.1` and emits
  each NDJSON line as a WebSocket text frame to clients that open a
  WS connection to the same port.

Both front-ends are built; the build target picks which one wash dev
serves (see `.wash/config.yaml`).

```
┌────────┐  GET /            ┌──────────────────────────┐  patches.subscribe()    ┌──────────────────┐
│ curl   │ ────────────────▶ │ patch-consumer           │ ──────────────────────▶ │ patch-producer   │
│  -N    │ ◀── chunked ───── │ (wasi:http/handler@0.3)  │ ◀── stream<u8> NDJSON ──│                  │
└────────┘  one HTTP chunk   │                          │                         │ spawns async     │
            per patch line   │ pipes stream straight    │                         │ writer task that │
                             │ into the response body   │                         │ paces 20 patches │
                             │ (zero-copy, no parsing)  │                         │ at 500ms cadence │
                             └──────────────────────────┘                         └──────────────────┘
                                  exports wasi:http/handler                       exports patches
                                  imports patches                                 (interface)
                                                                                  imports wasi:clocks/
                                                                                  monotonic-clock@0.3
```

The producer runs through a scripted 20-edit session against a tiny
JSON document, sleeping `wasi:clocks/monotonic-clock::wait-for(500ms)`
between writes and stamping each line with the elapsed wall time at
the moment it's produced (`[t+NNNms]`). That gives the response a
visible time axis end-to-end.

## Run

You need the locally-built wash at `../../target/debug/wash` (built
with `cargo build -p wash --features wasip3` from the wasmCloud repo
root). Both components need a recent nightly rustc for wit-bindgen's
wasi:http@0.3 custom sections.

```sh
../../target/debug/wash dev
```

Then:

```sh
$ curl -N http://localhost:8000/
[t+   0ms] {"op":"add","path":"/title","value":"\"Untitled\""}
[t+ 500ms] {"op":"add","path":"/version","value":"0"}
[t+1000ms] {"op":"add","path":"/items","value":"[]"}
[t+1500ms] {"op":"add","path":"/tags","value":"[]"}
[t+2000ms] {"op":"replace","path":"/title","value":"\"Streaming demo\""}
... (15 more lines, ~500 ms apart) ...
[t+9500ms] {"op":"replace","path":"/version","value":"4"}
```

The 500 ms spacing in the prefixes is the producer's per-write
`wait-for`. The end-to-end response takes ~9.5 s, and **curl
renders each line as it arrives** — the response uses HTTP/1.1
`Transfer-Encoding: chunked` end-to-end:

```
$ curl -sN -i http://localhost:8000/ | head -3
HTTP/1.1 200 OK
content-type: application/x-ndjson
transfer-encoding: chunked
```

This works because wash hands the wasi:http response body straight
through to hyper as a streaming `http_body::Body` (no `collect()`
buffering), and keeps `Store::run_concurrent` alive for the body's
full lifetime via a oneshot-signaled body wrapper — see patch #4
under "wash-runtime patches required to reach this point".

## WebSocket egress (meta-json)

The same producer feeds a WebSocket front-end. `meta-json` exports
`wasmcloud:websocket/handler@0.1` and, on each connection, subscribes
to `patches`, splits the NDJSON bytes on `\n`, and emits each line as
a `Frame::Text(...)` on the outbound stream.

```
┌────────┐  GET / Upgrade:    ┌─────────────────────────────┐  patches.subscribe()    ┌──────────────────┐
│ wscat  │  websocket ──────▶ │ meta-json                   │ ──────────────────────▶ │ patch-producer   │
│        │ ◀── WS text ────── │ (wasmcloud:websocket/handler│ ◀── stream<u8> NDJSON ──│                  │
└────────┘  frames, one per   │  @0.1.0)                    │                         │ (same as above)  │
            patch line, paced │                             │                         │                  │
            at producer rate  │ buffers bytes by '\n',      │                         │                  │
                              │ writes Frame::Text per line │                         │                  │
                              └─────────────────────────────┘                         └──────────────────┘
                                exports websocket/handler
                                imports patches
```

Switch `build.component_path` in `.wash/config.yaml` between
`patch_consumer.wasm` and `meta_json.wasm` to flip between the HTTP
and WebSocket front-ends. Both worlds keep producer as a peer.

### Try it

Handshake-only smoke test (works on any `curl`):

```sh
curl -sv --http1.1 \
  -H 'Connection: Upgrade' \
  -H 'Upgrade: websocket' \
  -H 'Sec-WebSocket-Version: 13' \
  -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
  http://localhost:8000/ 2>&1 | head -20
```

Expected response (the `accept` value is the canonical RFC 6455 test
vector — exact match means the host's SHA1 + base64 is correct):

```
HTTP/1.1 101 Switching Protocols
connection: Upgrade
upgrade: websocket
sec-websocket-accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=
```

For the full demo with proper frame parsing, use a real WS client
(`websocat`, `wscat`, or a browser):

```sh
websocat ws://localhost:8000/
# or
wscat -c ws://localhost:8000/
```

Expected output: 20 lines, one per WS text frame, ~500 ms apart:

```
[t+   0ms] {"op":"add","path":"/title","value":"\"Untitled\""}
[t+ 503ms] {"op":"add","path":"/version","value":"0"}
[t+1006ms] {"op":"add","path":"/items","value":"[]"}
...
[t+9500ms] {"op":"replace","path":"/version","value":"4"}
```

Then the host closes the connection with code 1000 (the producer's
writer drops → meta-json's outgoing stream ends → host sends a normal
WS close).

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
- **Single component, but linked to a peer for data.** meta-json
  itself only imports `patches`; the WS handler call goes through
  `pre_instantiate_linked_components_for_component` first so the
  patch-producer instance is available for meta-json's
  `patches::subscribe().await` to invoke.

### WS-handling code lives in

- WIT: [`wit/deps/wasmcloud-websocket-0.1.0/package.wit`](wit/deps/wasmcloud-websocket-0.1.0/package.wit)
  (vendored from `crates/wash-runtime/wit/deps/`).
- Host: [`crates/wash-runtime/src/host/websocket.rs`](../../crates/wash-runtime/src/host/websocket.rs).
- Dispatch branch: [`crates/wash-runtime/src/host/http.rs`](../../crates/wash-runtime/src/host/http.rs)
  (`is_websocket_upgrade` + the WS arm in `invoke_component_handler`).
- Workload binding: [`crates/wash-runtime/src/engine/workload.rs`](../../crates/wash-runtime/src/engine/workload.rs)
  — `resolve` now also registers WS-exporting components with the
  HTTP server so they receive incoming upgrade requests.

## What's actually being exercised

- **Cross-component p3 stream values flow through wash's dynamic
  linker.** The consumer's `patches::subscribe()` call invokes the
  producer's exported function via wash's
  `linker_instance.func_new_concurrent` bridge; the returned
  `StreamReader<u8>` handle crosses through wash's value
  lift/lower into the consumer.
- **Producer background tasks are pumped on the same concurrent
  runtime.** The producer's `wit_bindgen::spawn(async move { writer
  .write_all(...) })` writer task continues running after
  `subscribe` returns its stream handle, because the consumer's
  invocation went through `Store::run_concurrent` (set up by
  `host/http_p3.rs`) and the cross-component bridge uses
  `call_concurrent` rather than `call_async`.
- **Zero-copy stream forwarding.** Both `patches::subscribe` and
  `wasi:http/handler` use `stream<u8>`, so `Response::new(headers,
  Some(patches_rx), trailers_rx)` hands the patches stream straight
  in as the HTTP body — no copy task on the consumer side.
- **The producer paces and timestamps writes.** Between writes the
  producer awaits `wasi:clocks/monotonic-clock::wait-for(500ms)` —
  that's an `async func` import on a wasip3 host interface, and the
  await actually suspends the writer task without blocking the
  consumer's HTTP handler. The `[t+NNNms]` prefix is captured at
  the moment of `write_all`, so the response visibly records the
  producer's rhythm.
- **End-to-end chunked egress.** The response is sent as
  `Transfer-Encoding: chunked`; each `writer.write_all(line)` in
  the producer becomes one HTTP chunk on the wire, flushed by
  hyper as soon as the wasi:http pipe consumer hands it over.
  No collect, no Content-Length, no buffering between guest and
  client — `curl -N` shows lines arrive at the producer's cadence.

## Why the timestamps are stamped in the producer, not the consumer

A natural alternative would be: read each patch on the consumer
side with `patches_rx.next().await`, stamp it with the consumer's
clock at the moment it arrives, then push the timestamped line into
a *new* `stream<u8>` and hand that as the response body. The reading
side is fine — `.next()` / `.read(buf)` / `.collect()` all work on
the cross-component reader. The blocker is the *writer* side.

In our consumer's WIT world there's only one `stream<u8>` type that
wit-bindgen actually emits canonical-ABI builtins for: the patches
stream. (Inspect the consumer wasm: there's `[stream-new-0]subscribe`
under `wasmcloud:patch-stream/patches@0.1.0` and the boilerplate
`[stream-*-unit]`, but nothing like `[stream-new-0][static]request.new`
or `response.new` — wit-bindgen only generates a vtable for stream
types the guest itself constructs.) So `bindings::wit_stream::new::<u8>()`
in the consumer routes through the patches stream's slot, not a
wasi:http response-body slot. Handing that `body_rx` to
`Response::new` produces a slot-mismatch and the writer traps with

```
write pointer out of bounds
```

at the first `body_tx.write_all` — the writer's lowered length is
computed against the wrong type entry. Same family of bug as the
cross-component `Instance::copy` issue documented below, just
surfaced via a different lookup.

Pushing the timestamping into the producer dodges this entirely:
the producer already holds a writer for the patches stream (it
created the stream with `wit_stream::new::<u8>()` against its
own export's vtable) and has `wasi:clocks/monotonic-clock@0.3`
imported, so stamping each line at write time costs nothing extra.

## Why `stream<u8>` instead of `stream<patch>`

This was the design we wanted; it's blocked by an upstream wasmtime
bug that only surfaces under dynamic linking (i.e. wash's runtime).

The lift/lower of the stream return value across wash's bridge
works fine — verified by trace: `subscribe()` returns
`Stream(StreamAny { id: TransmitHandle(N), ty:
Guest(StreamType(TypeStreamIndex(M))) })`, wash identity-passes it,
and the consumer receives a stream handle without error. Likewise
the per-stream canonical-ABI builtins (`[stream-new-0]subscribe`,
`[stream-read-0]subscribe`, …) are wasmtime-compiled trampolines,
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
producer's `guest_write` fired. It's then resolved against the
**reader's** `ComponentTypes` table — which only happens to be
meaningful when both ends live in the same composed component
graph (one shared `ComponentTypes`).

Under wash's dynamic linking, producer and consumer are **separate
`Component`s with separate `ComponentTypes`**. Looking up the
producer's slot index in the consumer's type table returns whatever
the consumer happens to have at that slot — usually the body
stream's `stream<u8>`. `copy()` then lifts each item as `u8` and
tries to store it as the consumer's expected payload, producing:

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

This example would not run on stock wash; it requires four changes
in `crates/wash-runtime/`:

1. **[`engine/value.rs`](../../crates/wash-runtime/src/engine/value.rs)** —
   `lift`/`lower` for `Val::Stream`, `Val::Future`,
   `Val::ErrorContext` now return identity instead of bailing with
   `"async not supported"`. Single workload `Store` ⇒ the handle is
   valid on both sides.

2. **[`engine/workload.rs`](../../crates/wash-runtime/src/engine/workload.rs)** —
   `resolve_component_imports` registers cross-component imports via
   `linker_instance.func_new_concurrent` (was `func_new_async`) so
   the producer's `wit_bindgen::spawn` writer task gets polled on
   the same concurrent runtime as the consumer's `Store::run_concurrent`.
   The new `ResolvedWorkload::pre_instantiate_exporters` populates
   a per-store cache of exporter `Instance`s, since
   `pre.instantiate_async` cannot run inside the `accessor.with`
   sync closure that `func_new_concurrent` provides.

3. **[`host/http.rs`](../../crates/wash-runtime/src/host/http.rs)** —
   `invoke_component_handler` calls
   `workload_handle.pre_instantiate_exporters(&mut store, component_id)`
   right after `new_store` and before dispatching to the p2 or p3
   HTTP handler (both of which permanently flip the store into
   async-required state and prevent the linker bridge from
   instantiating exporters itself).

4. **[`host/http_p3.rs`](../../crates/wash-runtime/src/host/http_p3.rs)** —
   Stream the wasi:http response body straight through to hyper
   instead of `body.collect().await`-ing it. The handler future is
   spawned onto tokio; inside its `Store::run_concurrent`, we hand
   hyper a `StreamingBody` wrapper via a `oneshot` and then await a
   completion signal from the wrapper's `Drop` / final
   `poll_frame -> None`. That keeps the concurrent runtime alive
   for the body's lifetime so the wasi-http pipe consumer keeps
   pumping chunks into the body's mpsc. Hyper sees an unbounded
   `size_hint` and picks `Transfer-Encoding: chunked` on its own —
   no flag, no config — and curl renders each patch as it arrives
   at the producer's ~500ms cadence.

## Layout

```
examples/patch-stream/
├── Cargo.toml                # workspace: [patch-producer, patch-consumer, meta-json]
├── .wash/config.yaml         # wash dev: wasip3 on, patch-producer as peer component;
│                             # build.component_path selects the front-end (consumer | meta-json)
├── wit/
│   ├── world.wit             # patches + sink + three worlds (producer, consumer, meta-json)
│   └── deps/                 # wasi:http@0.3 + clocks@0.3 + wasmcloud:websocket@0.1 + ...
├── patch-producer/           # exports patches.subscribe; imports wasi:clocks/monotonic-clock@0.3
│                             # writes 20 timestamped NDJSON patches with 500 ms wait-for between
├── patch-consumer/           # exports wasi:http/handler@0.3, imports patches + sink
│                             # handle() = subscribe + hand off via sink::send-stream
└── meta-json/                # exports wasmcloud:patch-stream/sink AND
                              # wasmcloud:websocket/handler@0.1; imports patches.
                              # WS path: subscribe + emit each line as Frame::Text.
                              # sink path: drain stream<u8> + eprintln each line.
```

`.wash/config.yaml` sets `dev.wasip3: true` and lists
`patch-producer` under `dev.components` so wash dev loads it as a
peer of the entry consumer (the build target).

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
  consumer's world declares more than one `stream<T>` for the same
  `T`, `wit_stream::new::<T>()` can route through the wrong slot
  (see "Why the timestamps are stamped in the producer"). The
  workaround is to do the work in a component that owns the right
  vtable; a cleaner fix is upstream in wit-bindgen.
