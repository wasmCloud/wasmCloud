//! A p3 service co-driving `wasi:cli/run@0.3` (a p3 async tick loop), the p2
//! `wasmcloud:messaging/handler@0.2.0`, and `wasi:http/handler@0.3` on one
//! long-lived instance.
//!
//! `handle-message` increments a process-global `MSG_COUNT` and echoes
//! `"{count}:{subject}"` as its error result (observed directly by the trigger service
//! spike). The http handler reports the live `MSG_COUNT` as `{"count":N}`, so an
//! end-to-end test can publish a message through a messaging backend and read
//! the count over HTTP — proving the message reached the handler on the SAME
//! long-lived instance the trigger service co-drives, not a fresh one per message.

mod bindings;

use std::sync::atomic::{AtomicU64, Ordering};

use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::exports::wasmcloud::messaging::handler::Guest as MsgGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::messaging::types::BrokerMessage;

static MSG_COUNT: AtomicU64 = AtomicU64::new(0);

struct Component;

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        use bindings::wasi::clocks::monotonic_clock;
        // Keep the service alive; the trigger service co-drives this concurrently with
        // message and HTTP handling.
        loop {
            monotonic_clock::wait_for(1_000_000).await;
        }
    }
}

impl MsgGuest for Component {
    fn handle_message(msg: BrokerMessage) -> Result<(), String> {
        // A `boom` subject traps the handler, faulting the co-driven instance —
        // the restart-behavior test uses this to force a supervised restart.
        if msg.subject == "boom" {
            panic!("msg-counter boom: deliberate handler trap for the restart test");
        }
        let n = MSG_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        // Echo the running count + subject so the trigger service spike can observe
        // delivery and that the instance is long-lived.
        Err(format!("{n}:{}", msg.subject))
    }
}

impl HttpGuest for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let count = MSG_COUNT.load(Ordering::SeqCst);
        let body = format!("{{\"count\":{count}}}");
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
