use std::io::Write;

use wasmcloud_component::wasi::http::types::*;

/// Helper function to send an HTTP response
///
/// Sending an HTTP response with `wasi:http` involves creating an `OutgoingResponse` object,
/// setting the status code, and writing the response body. This function abstracts that process
/// into a single function call for convenience.
pub(crate) fn send_http_response(
    response_out: ResponseOutparam,
    status_code: u16,
    body: impl AsRef<[u8]>,
) {
    let response = OutgoingResponse::new(Fields::new());
    response
        .set_status_code(status_code)
        .expect("failed to set status code on response");
    let response_body = response.body().expect("failed to get response body");

    ResponseOutparam::set(response_out, Ok(response));
    response_body
        .write()
        .expect("failed to write to response body")
        .write_all(body.as_ref())
        .expect("failed to write to response body");

    OutgoingBody::finish(response_body, None).expect("failed to finish response body");
}

/// Splices the input stream into the output stream, copying all data from the input to the output.
pub(crate) fn splice(
    input: &wasi::io::streams::InputStream,
    output: wasi::io::streams::OutputStream,
) -> Result<(), wasi::io::error::Error> {
    loop {
        match output.blocking_splice(&input, u64::MAX) {
            Ok(0) | Err(wasi::io::streams::StreamError::Closed) => break,
            Err(wasi::io::streams::StreamError::LastOperationFailed(e)) => return Err(e),
            _ => {}
        }
    }
    Ok(())
}
