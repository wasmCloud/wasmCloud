# patch-stream

Proof-of-concept that two wasmCloud components can communicate over a
wasip3 cross-component `stream`, where a producer emits JSON Patch
records (RFC 6902, NDJSON-encoded) into a `stream<u8>` and a consumer
hands that stream straight through as the body of an HTTP response.

```
┌────────┐  GET /            ┌──────────────────────────┐  patches.subscribe()    ┌──────────────────┐
│ curl   │ ────────────────▶ │ patch-consumer           │ ──────────────────────▶ │ patch-producer   │
│  -N    │ ◀── chunked ───── │ (wasi:http/handler@0.3)  │ ◀── stream<u8> NDJSON ──│                  │
└────────┘  one HTTP chunk   │                          │                         │ spawns async     │
            per patch line   │ pipes stream straight    │                         │ writer task that │
                             │ into the response body   │                         │ paces 20 patches │
                             │ (zero-copy, no parsing)  │                         │ at 120 ms cadence│
                             └──────────────────────────┘                         └──────────────────┘
                                  exports wasi:http/handler                       exports patches
                                  imports patches                                 (interface)
                                                                                  imports wasi:clocks/
                                                                                  monotonic-clock@0.3
```

The producer runs through a scripted 20-edit session against a tiny
JSON document, sleeping `wasi:clocks/monotonic-clock::wait-for(120ms)`
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
[t+ 120ms] {"op":"add","path":"/version","value":"0"}
[t+ 240ms] {"op":"add","path":"/items","value":"[]"}
[t+ 360ms] {"op":"add","path":"/tags","value":"[]"}
[t+ 480ms] {"op":"replace","path":"/title","value":"\"Streaming demo\""}
... (15 more lines, ~120 ms apart) ...
[t+2400ms] {"op":"replace","path":"/version","value":"4"}
```

The 120 ms spacing in the prefixes is the producer's per-write
`wait-for`. The end-to-end response takes ~2.4 s, and **curl
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
  producer awaits `wasi:clocks/monotonic-clock::wait-for(120ms)` —
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
   at the producer's ~120ms cadence.

## Layout

```
examples/patch-stream/
├── Cargo.toml                # workspace: [patch-producer, patch-consumer]
├── .wash/config.yaml         # wash dev: wasip3 on, patch-producer as peer component
├── wit/
│   ├── world.wit             # patches interface + two worlds
│   └── deps/                 # wasi:http@0.3 + clocks@0.3 + transitive deps
├── patch-producer/           # exports patches.subscribe; imports wasi:clocks/monotonic-clock@0.3
│                             # writes 20 timestamped NDJSON patches with 120 ms wait-for between
└── patch-consumer/           # exports wasi:http/handler@0.3, imports patches
                              # handle() = subscribe + zero-copy hand-off into Response::new
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
