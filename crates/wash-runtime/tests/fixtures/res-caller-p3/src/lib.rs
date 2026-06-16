//! P3 fixture (HTTP entrypoint): obtains a `token` from `res-producer-p3` and
//! hands it to `res-sink-p3`, then returns the sink's reply as the HTTP body.
//! Passing the token to `accept` is what exercises `lower_with_type`'s
//! resource-identity passthrough across the dynamic linker.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "import:wasmcloud:resource-test/factory@0.1.0#make-token",
            "import:wasmcloud:resource-test/sink@0.1.0#accept",
            "export:wasi:http/handler@0.3.0-rc-2026-03-15#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::resource_test::{factory, sink};

struct Component;

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        // make_token returns a host-owned handle; passing it to accept lowers
        // it across the linker into res-sink-p3.
        let token = factory::make_token("world".to_string()).await;
        let body = sink::accept(token).await;

        let headers = Fields::new();
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());
        wit_bindgen::spawn(async move {
            tx.write_all(body.into_bytes()).await;
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
