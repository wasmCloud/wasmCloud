//! Self-contained P3 HTTP handler that emits its response body as a
//! sequence of chunks spaced out over time (via monotonic-clock
//! `wait-for`).
//!
//! It exists to prove the runtime *streams* P3 response bodies through to
//! hyper rather than buffering them. With streaming, a client reading the
//! body sees `chunk-0` hundreds of milliseconds before `chunk-9`. With the
//! old collect-then-send path, the handler would not return a response at
//! all until the whole body was produced, so the first byte and the last
//! byte arrive at essentially the same time.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "export:wasi:http/handler@0.3.0#handle",
            "import:wasi:clocks/monotonic-clock@0.3.0#wait-for",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

/// Number of chunks and the gap between them. 10 × 100ms ≈ 0.9s of paced
/// output, so a streaming client observes chunk 0 well before chunk 9.
const CHUNKS: u32 = 10;
const TICK_NS: u64 = 100_000_000; // 100ms

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let headers = Fields::new();
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

        // Emit `chunk-{i}\n` once per tick. The response is returned to the
        // host immediately; this task keeps feeding the body afterwards.
        wit_bindgen::spawn_local(async move {
            for i in 0..CHUNKS {
                if i > 0 {
                    monotonic_clock::wait_for(TICK_NS).await;
                }
                tx.write_all(format!("chunk-{i}\n").into_bytes()).await;
            }
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
