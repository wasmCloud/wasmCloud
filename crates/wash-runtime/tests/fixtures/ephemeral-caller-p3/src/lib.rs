//! Minimal P3 fixture: an HTTP handler that invokes the plain-value async
//! `run` export of the linked `ephemeral-callee-p3` component and returns the
//! result as the response body. Because `run`'s signature is all plain values,
//! the call is dispatched through the ephemeral-store path
//! (`invoke_ephemeral_linked_export`): the callee is instantiated in a
//! short-lived store that is dropped as soon as the call returns.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "import:wasmcloud:ephemeral-test/compute@0.1.0#run",
            "import:wasmcloud:ephemeral-test/compute@0.1.0#calls",
            "export:wasi:http/handler@0.3.0#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::ephemeral_test::compute;

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        // Plain-value async cross-component call -> ephemeral-store path.
        // `/calls` reports how many calls the callee instance has served, so a
        // caller can tell a reused instance from a fresh one; anything else is
        // run(21) = 21 * 2 + 1 = 43.
        let path = request.get_path_with_query().unwrap_or_default();
        let result = if path.starts_with("/calls") {
            compute::calls().await
        } else {
            compute::run(21).await
        };

        let headers = Fields::new();
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

        let body = result.to_string().into_bytes();
        wit_bindgen::spawn_local(async move {
            tx.write_all(body).await;
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
