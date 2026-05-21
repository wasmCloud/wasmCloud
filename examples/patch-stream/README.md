# patch-stream

Small wasmCloud POC for an agent-style streaming flow:

```text
curl prompt -> commander -> page-agent -> stream<u8> -> meta-json -> websocket client
```

The important thing this proves is that one request can open a WebSocket and
wait, while a later HTTP request triggers another component to generate a stream
that is forwarded to that already-connected WebSocket.

## Components

- `commander` receives `GET /?prompt=...` over `wasi:http/handler@0.3`.
- `page-agent` receives the prompt and returns a `stream<u8>`.
- `meta-json` accepts that stream and forwards each line to WebSocket clients.
- `wasmcloud:patch-stream/broker` is a wash-runtime host plugin that lets the
  HTTP path and WebSocket path rendezvous inside the same workload.

`page-agent` calls OpenAI when a key is configured. If no key is configured, it
falls back to a deterministic local stream so the demo still works.

## Quick Start

For the full local checklist, see [`DEVELOPMENT.md`](DEVELOPMENT.md).

1. Put your OpenAI API key in [`page-agent/src/lib.rs`](page-agent/src/lib.rs):

   ```rust
   const OPENAI_API_KEY: &str = "sk-...";
   ```

2. Build the components:

   ```sh
   cargo +nightly build --workspace --target wasm32-wasip2 --release
   ```

3. Run the local wash dev host:

   ```sh
   ../../target/debug/wash dev
   ```

4. In a second terminal, connect a WebSocket client first:

   ```sh
   websocat ws://localhost:8000/
   ```

5. In a third terminal, send the prompt:

   ```sh
   curl -i 'http://localhost:8000/?prompt=Make%20a%20landing%20page%20for%20streaming%20agents'
   ```

`curl` should return an empty `200 OK`. The generated JSON Patch lines should
appear in the `websocat` terminal.

## Expected Output

With OpenAI configured, `websocat` receives generated patch lines like:

```json
{"op":"add","path":"/title","value":"Welcome to Streaming Agents!"}
{"op":"add","path":"/description","value":"..."}
```

Without an OpenAI key, `page-agent` emits a local fallback stream with timing
prefixes so streaming cadence is obvious:

```text
[t+   0ms] {"op":"add","path":"/title","value":"\"Untitled\""}
[t+ 500ms] {"op":"add","path":"/version","value":"0"}
```

## Why Two Terminals?

The WebSocket request and the prompt request are intentionally separate:

- `websocat` opens the WebSocket and waits.
- `curl` asks `commander` to generate a page.
- `commander` calls `page-agent`, receives a `stream<u8>`, and passes it to
  `meta-json`.
- `meta-json` publishes each line to the waiting WebSocket clients.

This mirrors the chatbot-style shape we want later: a UI socket is already
connected, then another command triggers streamed agent output into that socket.

## Useful Files

- [`DEVELOPMENT.md`](DEVELOPMENT.md) - exact local run commands.
- [`wit/world.wit`](wit/world.wit) - component contracts.
- [`commander/src/lib.rs`](commander/src/lib.rs) - HTTP trigger component.
- [`page-agent/src/lib.rs`](page-agent/src/lib.rs) - OpenAI/fallback stream producer.
- [`meta-json/src/lib.rs`](meta-json/src/lib.rs) - stream sink and WebSocket handler.
- [`../../crates/wash-runtime/src/plugin/wasmcloud_stream_broker.rs`](../../crates/wash-runtime/src/plugin/wasmcloud_stream_broker.rs) - host broker plugin.
