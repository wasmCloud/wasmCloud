//! A p3 HTTP handler on the async runtime wit-bindgen shipped before 0.59.
//!
//! Every request awaits the clock once, so the task suspends and is woken by
//! something other than the runtime's wakeup stream: the case that runtime's
//! cancel order traps on. A reply means the host repaired it at load.

mod bindings;

use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

impl HttpGuest for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        monotonic_clock::wait_for(1_000_000).await;

        let (mut tx, rx) = bindings::wit_stream::new();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
        wit_bindgen::spawn_local(async move {
            tx.write_all(b"awaited".to_vec()).await;
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });
        let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
