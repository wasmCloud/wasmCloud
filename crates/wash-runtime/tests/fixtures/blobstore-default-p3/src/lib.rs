//! Real-guest fixture for a PLAIN (unlabeled) async `wasmcloud:blobstore` import.
//!
//! Unlike `blobstore-implements-p3`, this imports `wasmcloud:blobstore@0.1.0`
//! *without* an `(implements ..)` label. On each HTTP request it creates a
//! container, streams an object body in via `write-data(name, stream<u8>)`,
//! reads it back out via `get-data(..) -> stream<u8>`, and returns the bytes —
//! proving the host binds a default backend for a plain import (no label needed).

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::blobstore::blobstore;

struct Component;

const CONTAINER: &str = "photos";
const OBJECT: &str = "cat.png";
const BODY: &[u8] = b"woof from a plain p3 guest";

fn internal(msg: String) -> ErrorCode {
    ErrorCode::InternalError(Some(msg))
}

fn respond(status: u16, body_bytes: Vec<u8>) -> Result<Response, ErrorCode> {
    let headers = Fields::new();
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

    wit_bindgen::spawn_local(async move {
        tx.write_all(body_bytes).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });

    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    response
        .set_status_code(status)
        .map_err(|()| internal("failed to set status".into()))?;
    Ok(response)
}

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        // Create (or reuse) the container through the PLAIN blobstore import —
        // no label, so the host must route it to a default backend.
        let container = match blobstore::create_container(CONTAINER.to_string()).await {
            Ok(c) => c,
            Err(_) => blobstore::get_container(CONTAINER.to_string())
                .await
                .map_err(|e| internal(format!("get_container: {e:?}")))?,
        };

        // Stream the object body in through `write-data(name, stream<u8>)`.
        let (mut tx, rx) = bindings::wit_stream::new();
        wit_bindgen::spawn_local(async move {
            tx.write_all(BODY.to_vec()).await;
            drop(tx);
        });
        container
            .write_data(OBJECT.to_string(), rx)
            .await
            .map_err(|e| internal(format!("write_data: {e:?}")))?;

        // Read it back out of the returned `stream<u8>`.
        let stream = container
            .get_data(OBJECT.to_string(), 0, u64::MAX)
            .await
            .map_err(|e| internal(format!("get_data: {e:?}")))?;
        let data = stream.collect().await;

        respond(200, data)
    }
}

bindings::export!(Component with_types_in bindings);
