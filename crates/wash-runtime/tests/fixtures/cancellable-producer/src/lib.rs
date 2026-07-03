//! P3 cancellation fixture (producer side).
//!
//! Exports `produce`, returning a cross-component `stream<u8>` of `n` numbers
//! emitted one per second (`"1\n"`, `"2\n"`, ...). Paired with
//! `cancellable-component`, which imports this interface through the dynamic
//! linker and forwards the stream to an HTTP response body.
//!
//! The one-per-second pacing keeps the consumer's streaming invocation alive
//! for ~`n` seconds, giving `integration_p3_cancellation` a wide window to
//! cancel it mid-stream and observe the body stop early.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "export:wasmcloud:cancel-example/producer#produce",
            "import:wasi:clocks/monotonic-clock@0.3.0#wait-for",
        ],
    });
}

use bindings::exports::wasmcloud::cancel_example::producer::Guest;
use bindings::wasi::clocks::monotonic_clock;
use wit_bindgen::StreamReader;

struct Component;

/// Gap between emitted numbers.
const TICK_NS: u64 = 1_000_000_000; // 1s

impl Guest for Component {
    async fn produce(n: u32) -> StreamReader<u8> {
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        // Emit n numbers ("1\n", "2\n", ...) on a background task, one per
        // second, so the reader can be returned immediately and drained
        // incrementally by the consumer across the linker.
        wit_bindgen::spawn_local(async move {
            for i in 0..n {
                if i > 0 {
                    monotonic_clock::wait_for(TICK_NS).await;
                }
                tx.write_all(format!("{}\n", i + 1).into_bytes()).await;
            }
            drop(tx);
        });
        rx
    }
}

bindings::export!(Component with_types_in bindings);
