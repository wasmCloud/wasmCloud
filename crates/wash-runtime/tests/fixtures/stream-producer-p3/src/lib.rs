//! Minimal P3 fixture: exports `produce`, returning a cross-component
//! `stream<u8>`. Paired with `stream-consumer-p3`, which imports this
//! interface through the dynamic linker and forwards the stream to an
//! HTTP response body.
//!
//! Bytes are emitted one per tick (via monotonic-clock `wait-for`) so a
//! reader on the far side of the linker observes the stream arriving
//! incrementally. That's what lets `integration_p3_streams` prove the
//! cross-component stream stays concurrent rather than being buffered at the
//! linker boundary.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "export:wasmcloud:stream-test/producer@0.1.0#produce",
            "import:wasi:clocks/monotonic-clock@0.3.0#wait-for",
        ],
    });
}

use bindings::exports::wasmcloud::stream_test::producer::Guest;
use bindings::wasi::clocks::monotonic_clock;
use wit_bindgen::StreamReader;

struct Component;

/// Gap between emitted bytes. Paced so a reader across the dynamic linker
/// sees early bytes well before late ones; 16 bytes at this tick spans
/// ~0.75s, plenty to distinguish streaming from buffering.
const TICK_NS: u64 = 50_000_000; // 50ms

impl Guest for Component {
    async fn produce(n: u32) -> StreamReader<u8> {
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        // Emit `n` bytes ('a', 'b', ...) on a background task, one per tick,
        // so the reader can be returned immediately and drained incrementally
        // by the consumer across the linker.
        wit_bindgen::spawn(async move {
            for i in 0..n {
                if i > 0 {
                    monotonic_clock::wait_for(TICK_NS).await;
                }
                tx.write_all(vec![b'a' + (i % 26) as u8]).await;
            }
            drop(tx);
        });
        rx
    }
}

bindings::export!(Component with_types_in bindings);
