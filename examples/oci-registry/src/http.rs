//! Response construction and request-body handling over `wasi:http@0.3.0`.

use crate::bindings;
use crate::{Fields, Request, Response};

/// Read a header's first value as a UTF-8 string.
pub(crate) fn header_str(headers: &Fields, name: &str) -> Option<String> {
    headers
        .get(name)
        .into_iter()
        .next()
        .and_then(|value| String::from_utf8(value).ok())
}

/// Consume the request body into memory.
pub(crate) async fn read_request_body(request: Request) -> Vec<u8> {
    // `res` lets the guest signal an error back upstream; we always succeed, so
    // dropping the writer resolves it to `Ok(())` via the default.
    let (res_tx, res_rx) = bindings::wit_future::new(|| Ok(()));
    let (body, _trailers) = Request::consume_body(request, res_rx);
    let data = body.collect().await;
    drop(res_tx);
    data
}

/// Build a response with borrowed header pairs.
pub(crate) fn respond(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> Response {
    let fields = Fields::new();
    for &(name, value) in headers {
        let _ = fields.append(name, value.as_bytes());
    }
    finish_response(status, fields, body)
}

/// Build a response with owned header pairs (used where header values are
/// computed `String`s).
pub(crate) fn respond_owned(
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
) -> Response {
    let fields = Fields::new();
    for (name, value) in &headers {
        let _ = fields.append(name, value.as_bytes());
    }
    finish_response(status, fields, body)
}

fn finish_response(status: u16, headers: Fields, body: Vec<u8>) -> Response {
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        if !body.is_empty() {
            tx.write_all(body).await;
        }
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    let _ = response.set_status_code(status);
    response
}

/// Build a response whose body is forwarded from an existing byte stream (e.g.
/// a blobstore `get-data` reader), so the payload is never buffered in the guest.
pub(crate) fn stream_response(
    status: u16,
    headers: Vec<(String, String)>,
    body: wit_bindgen::StreamReader<u8>,
) -> Response {
    let fields = Fields::new();
    for (name, value) in &headers {
        let _ = fields.append(name, value.as_bytes());
    }
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(fields, Some(body), trailers_rx);
    let _ = response.set_status_code(status);
    response
}

pub(crate) fn error_response(status: u16, code: &str, message: &str) -> Response {
    let body = serde_json::json!({
        "errors": [{ "code": code, "message": message }]
    })
    .to_string();
    respond(
        status,
        &[("content-type", "application/json")],
        body.into_bytes(),
    )
}

pub(crate) fn method_not_allowed() -> Response {
    error_response(405, "UNSUPPORTED", "method not allowed for this route")
}
