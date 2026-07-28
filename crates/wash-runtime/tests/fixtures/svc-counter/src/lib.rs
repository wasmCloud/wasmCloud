//! A p3 service that co-drives `cli/run` and `http/handler` on one instance.
//!
//! `cli/run` increments a process-global `CLI_TICKS` on a fixed clock interval.
//! `http/handler` returns the live `cli_ticks` plus a per-call `http_calls`
//! count. Because both run on the same instance sharing the same statics, an
//! HTTP response observing `cli_ticks > 0` (and growing between requests) proves
//! the host co-drives the run loop concurrently with serving HTTP.

mod bindings;

use std::sync::atomic::{AtomicU64, Ordering};

use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

static CLI_TICKS: AtomicU64 = AtomicU64::new(0);
static HTTP_CALLS: AtomicU64 = AtomicU64::new(0);

struct Component;

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        use bindings::wasi::clocks::monotonic_clock;
        loop {
            // 10ms tick — fast enough that any HTTP request arrives after >0.
            monotonic_clock::wait_for(10_000_000).await;
            CLI_TICKS.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl HttpGuest for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let http_calls = HTTP_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        let cli_ticks = CLI_TICKS.load(Ordering::SeqCst);
        let body = format!("{{\"cli_ticks\":{cli_ticks},\"http_calls\":{http_calls}}}");
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
