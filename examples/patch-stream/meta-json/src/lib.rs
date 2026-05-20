mod bindings {
    wit_bindgen::generate!({
        world: "meta-json",
        path: "../wit",
        generate_all,
        async: [
            "export:wasmcloud:patch-stream/sink@0.1.0#send-stream",
        ],
    });
}

use bindings::exports::wasmcloud::patch_stream::sink::Guest;
use wit_bindgen::StreamReader;

struct Component;

impl Guest for Component {
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

bindings::export!(Component with_types_in bindings);
