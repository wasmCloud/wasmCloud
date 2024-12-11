use anyhow::{anyhow, bail, ensure, Result};
use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::bindings::wasi::http::types::IncomingBody;
use crate::bindings::wasi::io::streams::StreamError;
use crate::incoming_handler::{IncomingRequest, ResponseOutparam};
use crate::{log, Fields, Level, OutgoingBody, OutgoingResponse, LOG_CONTEXT};

/// Maximum bytes to write at a time, due to the limitations on wasi-io's blocking_write_and_flush()
const MAX_WRITE_BYTES: usize = 4096;

/// Maximum bytes to read at a time from the incoming request body
/// this value is chosen somewhat arbitrarily, and is not a limit for bytes read,
/// but is instead the amount of bytes to be read *at once*
const MAX_READ_BYTES: usize = 2048;

/// `try_send_error` attempts to send an error
macro_rules! try_send_error {
    ($log_ctx:ident, $status_code:expr, $error_code:expr, $msg:expr, $r_out:expr $(,)?) => {
        if let Err(e) = crate::http::send_error_response($status_code, $error_code, $msg, $r_out) {
            crate::bindings::wasi::logging::logging::log(
                crate::bindings::wasi::logging::logging::Level::Error,
                $log_ctx,
                &format!("failed to send error response: {e}"),
            );
            panic!("failed to send error response");
        }
    };
}
pub(crate) use try_send_error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaskCompleteRequestBody {
    pub(crate) worker_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaskLeaseRequestBody {
    pub(crate) worker_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaskReleaseRequestBody {
    pub(crate) worker_id: String,
    pub(crate) lease_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaskFailedRequestBody {
    pub(crate) worker_id: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TaskSubmitRequestBody {
    pub(crate) group_id: String,
    pub(crate) task_data: serde_json::Value,
}

// NOTE: Since wit-bindgen makes `IncomingRequest` available to us as a local type,
// we can add convenience functions to it
impl IncomingRequest {
    /// Check if the incoming request body has the `application/json` content type
    pub(crate) fn is_json(&self) -> bool {
        self.headers()
            .entries()
            .iter()
            .find(|(k, _v)| k.to_lowercase() == "content-type")
            .is_some_and(|(_k, v)| String::from_utf8_lossy(v).contains("application/json"))
    }

    /// This is a convenience function that extracts out the body of a IncomingRequest
    pub(crate) fn read_body(self) -> Result<Bytes> {
        let incoming_req_body = self
            .consume()
            .map_err(|()| anyhow!("failed to consume incoming request body"))?;
        let incoming_req_body_stream = incoming_req_body
            .stream()
            .map_err(|()| anyhow!("failed to build stream for incoming request body"))?;
        let mut buf = BytesMut::with_capacity(MAX_READ_BYTES);
        loop {
            match incoming_req_body_stream.blocking_read(MAX_READ_BYTES as u64) {
                Ok(bytes) if bytes.is_empty() => break,
                Ok(bytes) => {
                    ensure!(
                        bytes.len() <= MAX_READ_BYTES,
                        "read more bytes than requested"
                    );
                    buf.extend(bytes);
                }
                Err(StreamError::Closed) => break,
                Err(e) => bail!("failed to read bytes: {e}"),
            }
        }
        drop(incoming_req_body_stream);
        IncomingBody::finish(incoming_req_body);
        Ok(buf.freeze())
    }
}

/// Utility function for building and sending an error response using the WASI HTTP `ResponseOutparam`
pub(crate) fn send_error_response(
    status: u16,
    code: &str,
    msg: &str,
    response_out: ResponseOutparam,
) -> Result<()> {
    send_response_json(
        response_out,
        json!({
            "status": "error",
            "error": {
                "code": code,
                "message": msg,
            }
        }),
        status,
    );
    Ok(())
}

/// Send a response back with JSON
pub(crate) fn send_response_json(
    response_out: ResponseOutparam,
    body: serde_json::Value,
    status: u16,
) {
    let bytes = Bytes::from(body.to_string());
    let Ok(headers) = Fields::from_list(&[(
        "Content-Type".into(),
        "application/json;charset=utf8".into(),
    )]) else {
        try_send_error!(
            LOG_CONTEXT,
            500,
            "unexpected-err",
            "failed to build response headers",
            response_out,
        );
        return;
    };

    let response = OutgoingResponse::new(headers);

    if let Err(()) = response.set_status_code(status) {
        try_send_error!(
            LOG_CONTEXT,
            500,
            "unexpected-err",
            "failed to set status code",
            response_out,
        );
        return;
    }

    let Ok(response_body) = response.body() else {
        try_send_error!(
            LOG_CONTEXT,
            500,
            "unexpected-err",
            "failed to get body from outgoing response",
            response_out,
        );
        return;
    };

    let Ok(stream) = response_body.write() else {
        try_send_error!(
            LOG_CONTEXT,
            500,
            "unexpected-err",
            "failed to get output stream from body",
            response_out,
        );
        return;
    };

    // Write the image bytes to the output stream
    for chunk in bytes.chunks(MAX_WRITE_BYTES) {
        if let Err(e) = stream
            .blocking_write_and_flush(chunk)
            .map_err(|e| anyhow!("failed to write chunk: {e}"))
        {
            try_send_error!(
                LOG_CONTEXT,
                500,
                "unexpected-err",
                &format!("failed write image chunk to output stream: {e}"),
                response_out,
            );
            return;
        }
    }

    ResponseOutparam::set(response_out, Ok(response));
    if let Err(e) = OutgoingBody::finish(response_body, None) {
        log(
            Level::Error,
            LOG_CONTEXT,
            &format!("failed to finish response body: {e}"),
        );
    }
}
