//! P3 cancellation fixture (consumer side).
//!
//! An HTTP handler that drives a *cancellable* streaming invocation:
//!   - `GET /?id=<id>` registers the current invocation with the host
//!     `cancellable-jobs/control` plugin under `<id>`, then pulls a paced
//!     `stream<u8>` from the linked `cancellable-producer` component and
//!     forwards it to the response body. The producer emits one number per
//!     second, so the invocation stays alive streaming for several seconds.
//!   - `GET /cancel?id=<id>` asks the host to cancel the invocation
//!     registered under `<id>`. That trips the invocation's epoch handle and
//!     the runtime traps its store mid-stream, so the forwarding above stops
//!     and the client's body ends early.
//!
//! The pair (`cancellable-producer` + this) plus `integration_p3_cancellation`
//! prove that an in-flight streaming invocation can be stopped on demand: with
//! a cancel at ~5s the client sees only the first few numbers, never all ten.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "import:wasmcloud:cancel-example/producer#produce",
            "export:wasi:http/handler@0.3.0#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::cancel_example::producer;
use bindings::wasmcloud::cancellable_jobs::control;

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request.get_path_with_query().unwrap_or_default();
        let Some(id) = extract_request_id(&path) else {
            return Ok(send_response(b"please send a request with an id\n".to_vec()));
        };

        if path.starts_with("/cancel") {
            let cancelled = control::cancel(&id);
            // This is asserted in test.
            let msg = if cancelled { "cancelled\n" } else { "not-registered\n" };
            return Ok(send_response(msg.as_bytes().to_vec()));
        }

        // Normal request route that registers and performs a normal long-running call
        if let Err(e) = control::register(&id) {
            return Ok(send_response(format!("register failed: {e:?}\n").into_bytes()));
        }

        // produce 10 numbers.
        let mut upstream = producer::produce(10).await;

        let headers = Fields::new();
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

        // Forward the bytes straight into the HTTP response body.
        // If this invocation is cancelled, the runtime traps the store and this task
        // dies mid-loop
        wit_bindgen::spawn_local(async move {
            while let Some(byte) = upstream.next().await {
                tx.write_all(vec![byte]).await;
            }
            drop(tx);
            if let Err(e) = trailers_tx.write(Ok(None)).await {
                tracing::debug!(?e, "trailers receivers gone; consumer likely disconnected");
            }
        });

        let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
        Ok(response)
    }
}

fn extract_request_id(path: &str) -> Option<String> {
    path.split_once('?')
        .and_then(|(_, query)| 
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix("id="))
        ).filter(|id| !id.is_empty()).map(str::to_string)
}

fn send_response(body: Vec<u8>) -> Response {
    let headers = Fields::new();
    let (mut tx, rx) = bindings::wit_stream::new::<u8>();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());
    wit_bindgen::spawn_local(async move {
        tx.write_all(body).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    response
}

bindings::export!(Component with_types_in bindings);
