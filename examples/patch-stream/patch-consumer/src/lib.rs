mod bindings {
    wit_bindgen::generate!({
        world: "patch-consumer",
        path: "../wit",
        generate_all,
        async: [
            "import:wasmcloud:patch-stream/patches@0.1.0#subscribe",
            "export:wasi:http/handler@0.3.0-rc-2026-03-15#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::patch_stream::patches;

struct Component;

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let headers = Fields::new();
        let _ = headers.append(
            &"content-type".to_string(),
            &b"application/x-ndjson".to_vec(),
        );

        // Both `patches::subscribe` and `wasi:http/handler` use stream<u8>,
        // so hand the patches stream straight to the response body — no
        // copy task needed. The `[t+NNNms]` prefix on each line is
        // baked in by the producer.
        let patches_rx = patches::subscribe().await;
        let (_trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));

        let (response, _result) = Response::new(headers, Some(patches_rx), trailers_rx);
        response
            .set_status_code(200)
            .map_err(|()| ErrorCode::InternalError(Some("set_status failed".into())))?;
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
