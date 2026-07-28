//! A p3 service with **no `wasi:cli/run` export** — only `http/handler`.
//!
//! `http/handler` returns a per-instance call count held in a process-global.
//! A service is instantiated once and serves every request as a concurrent
//! task on that one instance, so the count climbs across requests. A component
//! (the non-service path) is instantiated per request, so its count would read
//! `1` every time. The response therefore says which one the host did.
//!
//! Mirrors `svc-counter` minus its `cli/run` tick loop, so the pair isolates
//! exactly what `cli/run` is needed for.

mod bindings;

use std::sync::atomic::{AtomicU64, Ordering};

use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

static HTTP_CALLS: AtomicU64 = AtomicU64::new(0);

struct Component;

impl HttpGuest for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let http_calls = HTTP_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        let body = format!("{{\"http_calls\":{http_calls}}}");
        Ok(make_response(200, body.into_bytes()))
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

bindings::export!(Component with_types_in bindings);
