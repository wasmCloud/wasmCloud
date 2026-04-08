mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::blobstore::{blobstore, types::OutgoingValue};
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

const CONTAINER_NAME: &str = "test-container";
const OBJECT_KEY: &str = "test-object";

fn respond(status: u16, body_bytes: Vec<u8>) -> Result<Response, ErrorCode> {
    let headers = Fields::new();
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

    wit_bindgen::spawn(async move {
        tx.write_all(body_bytes).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });

    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    response
        .set_status_code(status)
        .map_err(|()| ErrorCode::InternalError(Some("failed to set status".into())))?;
    Ok(response)
}

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        // Create or get container
        let container = match blobstore::get_container(CONTAINER_NAME) {
            Ok(c) => c,
            Err(_) => match blobstore::create_container(CONTAINER_NAME) {
                Ok(c) => c,
                Err(e) => {
                    return respond(500, format!("failed to create container: {e}").into_bytes());
                }
            },
        };

        // Write data to blobstore
        let data = b"hello from p3 blobstore";
        let outgoing = OutgoingValue::new_outgoing_value();
        let stream = outgoing
            .outgoing_value_write_body()
            .map_err(|_| ErrorCode::InternalError(Some("failed to get write body".into())))?;
        stream
            .blocking_write_and_flush(data)
            .map_err(|_| ErrorCode::InternalError(Some("failed to write".into())))?;
        drop(stream);

        container
            .write_data(OBJECT_KEY, &outgoing)
            .map_err(|e| ErrorCode::InternalError(Some(format!("write_data failed: {e}"))))?;
        OutgoingValue::finish(outgoing).ok();

        // Read data back from blobstore
        let incoming = container
            .get_data(OBJECT_KEY, 0, u64::MAX)
            .map_err(|e| ErrorCode::InternalError(Some(format!("get_data failed: {e}"))))?;

        let read_data = bindings::wasi::blobstore::types::IncomingValue::incoming_value_consume_sync(incoming)
            .map_err(|_| ErrorCode::InternalError(Some("consume failed".into())))?;

        respond(200, read_data)
    }
}

bindings::export!(Component with_types_in bindings);
