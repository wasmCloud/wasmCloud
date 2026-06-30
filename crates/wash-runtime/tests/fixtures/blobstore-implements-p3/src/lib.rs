//! Real-guest fixture for async `wasmcloud:blobstore` `(implements ..)` routing.
//!
//! Imports the native-stream `wasmcloud:blobstore@0.1.0` under the labels
//! `store-a` (the `blobstore` interface) and `objects-a` (the `container`
//! interface). On each HTTP request it creates a container, streams an object
//! body in via `write-data(name, stream<u8>)`, reads it back out via
//! `get-data(..) -> stream<u8>`, and returns the bytes — exercising the
//! concurrent/stream host ABI end to end against whatever backend the host
//! routes `store-a` to.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

const CONTAINER: &str = "photos";
const OBJECT: &str = "cat.png";
const BODY: &[u8] = b"meow from a real p3 guest";

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
        // Create (or reuse) the container through the `store-a` implements import.
        let container = match bindings::store_a::create_container(CONTAINER.to_string()).await {
            Ok(c) => c,
            Err(_) => bindings::store_a::get_container(CONTAINER.to_string())
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
