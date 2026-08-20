//! A p3 trigger service that calls `wasi:cli/run` on a workload component.
//!
//! Each HTTP request invokes the imported `wasi:cli/run`, which the host routes
//! to the component exporting it, and answers with the outcome — driving it
//! from a request rather than a timer keeps the test deterministic.
//!
//! The service's own exported `cli/run` is an idle keep-alive loop, so the same
//! interface name is in play as both a co-driven export and a routed import.

mod bindings;

use std::sync::atomic::{AtomicU64, Ordering};

use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

/// Counts invocations this service has dispatched, so a test can tell a fresh
/// answer from a cached one.
static DISPATCHED: AtomicU64 = AtomicU64::new(0);

struct Component;

impl HttpGuest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request
            .get_path_with_query()
            .unwrap_or_else(|| "/".to_string());
        if !path.starts_with("/run") {
            return Ok(make_response(404, Vec::new()));
        }
        let outcome = bindings::wasi::cli::run::run().await;
        let dispatched = DISPATCHED.fetch_add(1, Ordering::SeqCst) + 1;
        let ok = outcome.is_ok();
        let body = format!("{{\"ok\":{ok},\"dispatched\":{dispatched}}}");
        Ok(make_response(200, body.into_bytes()))
    }
}

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        use bindings::wasi::clocks::monotonic_clock;
        // Keep the service alive; the co-driver runs this concurrently with HTTP.
        loop {
            monotonic_clock::wait_for(1_000_000).await;
        }
    }
}

fn make_response(status: u16, body: Vec<u8>) -> Response {
    let headers = Fields::new();
    let _ = headers.set(&"content-type".to_string(), &[b"application/json".to_vec()]);
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        tx.write_all(body).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    let _ = response.set_status_code(status);
    response
}

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
