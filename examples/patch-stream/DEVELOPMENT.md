# Development

This example needs two client terminals. The WebSocket client connects first
and waits. A separate HTTP request then asks `commander` to call `page-agent`,
and `meta-json` forwards the resulting stream to the waiting WebSocket.

## Add Your API Key

For the local POC, put your OpenAI API key in
`page-agent/src/lib.rs`:

```rust
const OPENAI_API_KEY: &str = "sk-...";
```

If the constant is empty, `page-agent` uses the deterministic fallback stream
instead of calling OpenAI.

## Build

From this directory:

```sh
cargo +nightly build --workspace --target wasm32-wasip2 --release
```

## Run Wash

From this directory:

```sh
../../target/debug/wash dev
```

Wait until wash prints:

```text
listening for HTTP requests address=http://127.0.0.1:8000
```

## Terminal 1: WebSocket Subscriber

Start this before sending the prompt. It should stay open and initially print
nothing:

```sh
websocat ws://localhost:8000/
```

## Terminal 2: Prompt Trigger

In another terminal, send a prompt to `commander`:

```sh
curl -i 'http://localhost:8000/?prompt=Make%20a%20landing%20page%20for%20streaming%20agents'
```

`curl` should return an empty `200 OK`. The generated patch lines should appear
in the `websocat` terminal as WebSocket text frames.

## Expected Flow

```text
websocat opens WS -> meta-json waits on broker
curl sends prompt -> commander calls page-agent
page-agent returns stream<u8> -> commander passes it to meta-json
meta-json publishes each line -> websocat receives frames
```
