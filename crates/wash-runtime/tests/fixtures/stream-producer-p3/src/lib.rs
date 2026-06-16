//! Minimal P3 fixture: exports `produce`, returning a cross-component
//! `stream<u8>`. Paired with `stream-consumer-p3`, which imports this
//! interface through the dynamic linker and forwards the stream to an
//! HTTP response body.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "export:wasmcloud:stream-test/producer@0.1.0#produce",
        ],
    });
}

use bindings::exports::wasmcloud::stream_test::producer::Guest;
use wit_bindgen::StreamReader;

struct Component;

impl Guest for Component {
    async fn produce(n: u32) -> StreamReader<u8> {
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        // Emit `n` bytes ('a', 'b', ...) on a background task so the reader
        // can be returned immediately and drained by the consumer.
        wit_bindgen::spawn(async move {
            for i in 0..n {
                tx.write_all(vec![b'a' + (i % 26) as u8]).await;
            }
            drop(tx);
        });
        rx
    }
}

bindings::export!(Component with_types_in bindings);
