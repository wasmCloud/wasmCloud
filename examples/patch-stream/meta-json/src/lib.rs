mod bindings {
    wit_bindgen::generate!({
        world: "meta-json",
        path: "../wit",
        generate_all,
        async: [
            "export:wasmcloud:patch-stream/sink@0.1.0#send-stream",
            "export:wasmcloud:websocket/handler@0.1.0#handle",
            "import:wasmcloud:patch-stream/patches@0.1.0#subscribe",
        ],
    });
}

use bindings::exports::wasmcloud::patch_stream::sink::Guest as SinkGuest;
use bindings::exports::wasmcloud::websocket::handler::Guest as WsGuest;
use bindings::wasmcloud::patch_stream::patches;
use bindings::wasmcloud::websocket::types::{Frame, UpgradeRequest};
use wit_bindgen::StreamReader;

struct Component;

// ---- Existing sink path (HTTP-NDJSON demo, kept for parity) ----

impl SinkGuest for Component {
    async fn send_stream(mut s: StreamReader<u8>) -> Result<(), ()> {
        eprintln!("meta-json: stream received, draining…");
        let mut line: Vec<u8> = Vec::with_capacity(256);
        let mut bytes: u64 = 0;
        let mut lines: u64 = 0;
        while let Some(byte) = s.next().await {
            bytes += 1;
            if byte == b'\n' {
                lines += 1;
                eprintln!(
                    "meta-json: [{lines:>3}] {}",
                    String::from_utf8_lossy(&line)
                );
                line.clear();
            } else {
                line.push(byte);
            }
        }
        if !line.is_empty() {
            lines += 1;
            eprintln!(
                "meta-json: [{lines:>3}] {} (no trailing newline)",
                String::from_utf8_lossy(&line)
            );
        }
        eprintln!("meta-json: stream closed after {bytes} bytes / {lines} patches");
        Ok(())
    }
}

// ---- WebSocket path ----

impl WsGuest for Component {
    async fn handle(
        req: UpgradeRequest,
        mut incoming: StreamReader<Frame>,
    ) -> Result<StreamReader<Frame>, String> {
        eprintln!(
            "meta-json[ws]: connection opened path={:?} subprotocols={:?}",
            req.path, req.subprotocols
        );

        let (mut outgoing_tx, outgoing_rx) = bindings::wit_stream::new::<Frame>();

        // Drain incoming frames in the background so the connection
        // stays observable bidirectionally. We only log; nothing in
        // this demo reacts to client-sent frames.
        wit_bindgen::spawn(async move {
            while let Some(frame) = incoming.next().await {
                match frame {
                    Frame::Text(s) => eprintln!("meta-json[ws]: recv text: {s}"),
                    Frame::Binary(b) => {
                        eprintln!("meta-json[ws]: recv binary {} bytes", b.len())
                    }
                    Frame::Close(info) => {
                        eprintln!(
                            "meta-json[ws]: recv close code={} reason={:?}",
                            info.code, info.reason
                        );
                        break;
                    }
                }
            }
            eprintln!("meta-json[ws]: incoming stream closed");
        });

        // Forward each NDJSON line from the patches stream as one text
        // frame. Patches stream closes when the producer drops its
        // writer; dropping our outgoing tx then signals the host to
        // close the WS cleanly.
        wit_bindgen::spawn(async move {
            let mut patches_rx = patches::subscribe().await;
            let mut line: Vec<u8> = Vec::with_capacity(256);
            let mut lines: u64 = 0;
            while let Some(byte) = patches_rx.next().await {
                if byte == b'\n' {
                    lines += 1;
                    let text = String::from_utf8_lossy(&line).into_owned();
                    outgoing_tx.write_all(vec![Frame::Text(text)]).await;
                    line.clear();
                } else {
                    line.push(byte);
                }
            }
            if !line.is_empty() {
                lines += 1;
                let text = String::from_utf8_lossy(&line).into_owned();
                outgoing_tx.write_all(vec![Frame::Text(text)]).await;
            }
            eprintln!("meta-json[ws]: sent {lines} text frames; closing");
            // outgoing_tx drops here → host closes WS with 1000.
        });

        Ok(outgoing_rx)
    }
}

bindings::export!(Component with_types_in bindings);
