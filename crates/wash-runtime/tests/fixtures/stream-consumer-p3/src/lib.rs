//! Minimal P3 fixture: an HTTP handler that pulls a `stream<u8>` from the
//! linked `stream-producer-p3` component and forwards it to the response
//! body. Exercises the cross-component stream path:
//!   - the `stream<u8>` handle crosses the dynamic linker boundary
//!     (`lower_with_type` resource-identity passthrough),
//!   - the producer is auto-linked into this workload
//!     (`component_ids_except`),
//!   - the response body streams through to hyper without being buffered.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "import:wasmcloud:stream-test/producer@0.1.0#produce",
            "export:wasi:http/handler@0.3.0-rc-2026-03-15#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::stream_test::producer;

struct Component;

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        // Pull a byte stream from the linked producer component. This
        // `stream<u8>` handle is created in the producer and travels across
        // the dynamic linker into this component.
        let mut upstream = producer::produce(16).await;

        let headers = Fields::new();
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

        // Forward producer bytes straight into the HTTP response body, then
        // close. The runtime streams P3 bodies through to hyper, so these
        // reach the client without being buffered in full first.
        wit_bindgen::spawn(async move {
            while let Some(byte) = upstream.next().await {
                tx.write_all(vec![byte]).await;
            }
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
