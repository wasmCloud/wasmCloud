mod bindings {
    wit_bindgen::generate!({
        world: "patch-consumer",
        path: "../wit",
        generate_all,
        async: [
            "import:wasmcloud:patch-stream/patches@0.1.0#subscribe",
            "import:wasmcloud:patch-stream/sink@0.1.0#send-stream",
            "export:wasi:http/handler@0.3.0-rc-2026-03-15#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::patch_stream::{patches, sink};

struct Component;

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let headers = Fields::new();
        let _ = headers.append(
            &"content-type".to_string(),
            &b"text/plain; charset=utf-8".to_vec(),
        );

        // Kick off the producer and hand the resulting stream off to
        // meta-json. We don't keep a reader for ourselves — the
        // commander's job is to dispatch, meta-json is responsible
        // for persisting / logging the result.
        let patches_rx = patches::subscribe().await;
        let result = sink::send_stream(patches_rx).await;

        let (_trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
        let (response, _result) = Response::new(headers, None, trailers_rx);
        let status = match result {
            Ok(()) => 200,
            Err(()) => 502,
        };
        response
            .set_status_code(status)
            .map_err(|()| ErrorCode::InternalError(Some("set_status failed".into())))?;
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
