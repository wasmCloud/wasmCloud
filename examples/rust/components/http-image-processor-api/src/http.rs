use std::collections::HashMap;

use anyhow::{anyhow, Result};
use bytes::{Buf, Bytes};
use image::ImageFormat;
use serde_json::json;

use crate::bindings::exports::wasi::http::incoming_handler::{IncomingRequest, ResponseOutparam};
use crate::{log, Fields, Level, OutgoingBody, OutgoingResponse, LOG_CONTEXT};

use crate::MAX_WRITE_BYTES;

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

/// Extract HTTP headers from an incoming request
pub(crate) fn extract_headers(req: &IncomingRequest) -> HashMap<String, Vec<String>> {
    let mut headers = HashMap::new();
    for (k, v) in req.headers().entries().into_iter() {
        match String::from_utf8(v) {
            Ok(v) => {
                headers
                    .entry(k)
                    .and_modify(|vs: &mut Vec<String>| vs.push(v.clone()))
                    .or_insert(vec![v.clone()]);
            }
            Err(e) => {
                log(
                    Level::Warn,
                    LOG_CONTEXT,
                    &format!("failed to parse value for header key [{k}] into string: {e}"),
                );
            }
        }
    }
    headers
}

/// Parse an [`ImageFormat`] from a string that vaguely looks like a MIME content type
pub(crate) fn fuzzy_parse_image_format_from_mime(s: &str) -> Option<ImageFormat> {
    if s.contains("image/jpeg") {
        return Some(ImageFormat::Jpeg);
    }
    if s.contains("image/jpg") {
        return Some(ImageFormat::Jpeg);
    }
    if s.contains("image/tiff") {
        return Some(ImageFormat::Tiff);
    }
    if s.contains("image/webp") {
        return Some(ImageFormat::WebP);
    }
    if s.contains("image/gif") {
        return Some(ImageFormat::Gif);
    }
    if s.contains("image/bmp") {
        return Some(ImageFormat::Bmp);
    }
    None
}

/// Things that can be built from headers
pub(crate) trait FromHttpHeader
where
    Self: Sized,
{
    /// Build a value of the implementing type from header
    fn from_header(k: &str, values: &Vec<String>) -> Option<Self>;
}

impl FromHttpHeader for ImageFormat {
    fn from_header(k: &str, values: &Vec<String>) -> Option<Self> {
        if k.to_lowercase() != "content-type" {
            return None;
        }

        values
            .iter()
            .map(String::as_str)
            .map(str::to_lowercase)
            .find_map(|s| fuzzy_parse_image_format_from_mime(&s))
    }
}

/// Things that can be built to headers
pub(crate) trait ToHttpHeader
where
    Self: Sized,
{
    /// Build a value of the implementing type to header
    fn to_header(&self) -> (&str, &str);
}

impl ToHttpHeader for ImageFormat {
    fn to_header(&self) -> (&str, &str) {
        (
            "Content-Type",
            match &self {
                ImageFormat::Jpeg => "image/jpeg",
                ImageFormat::Tiff => "image/tiff",
                ImageFormat::WebP => "image/webp",
                ImageFormat::Gif => "image/gif",
                ImageFormat::Bmp => "image/Bmp",
                _ => "application/octet-stream",
            },
        )
    }
}

/// Wrapper used to implement multipart
pub(crate) struct RequestBodyBytes<'a> {
    pub content_type: &'a str,
    pub body: Bytes,
}

impl multipart::server::HttpRequest for RequestBodyBytes<'_> {
    type Body = bytes::buf::Reader<Bytes>;

    fn multipart_boundary(&self) -> Option<&str> {
        self.content_type.find("boundary=").map(|idx| {
            let start = idx + "boundary=".len();
            let end = self.content_type[idx..]
                .find(";")
                .unwrap_or(self.content_type.len());
            return &self.content_type[start..end];
        })
    }

    fn body(self) -> bytes::buf::Reader<Bytes> {
        self.body.clone().reader()
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

/// Build and send a 200 OK HTTP response with bytes of an image
pub(crate) fn send_image_response(
    image_format: Option<ImageFormat>,
    image_bytes: Bytes,
    response_out: ResponseOutparam,
) {
    let headers = Fields::new();

    // Add the image format header if present
    if let Some(format) = image_format {
        let (key, value) = format.to_header();
        if let Err(e) = headers.set(&String::from(key), &[value.as_bytes().into()]) {
            if let Err(e) = send_error_response(
                500,
                "unexpected-error",
                &format!("failed to set content-type header on response: {e}"),
                response_out,
            ) {
                log(
                    Level::Error,
                    LOG_CONTEXT,
                    &format!("failed to send error response: {e}"),
                );
                panic!("failed to send error response");
            }
            return;
        }
    }

    let response = OutgoingResponse::new(headers);

    if let Err(()) = response.set_status_code(200) {
        if let Err(e) = send_error_response(
            500,
            "unexpected-error",
            "failed to set status code",
            response_out,
        ) {
            log(
                Level::Error,
                LOG_CONTEXT,
                &format!("failed to send error response: {e}"),
            );
            panic!("failed to send error response");
        }
        return;
    }

    let Ok(response_body) = response.body() else {
        if let Err(e) = send_error_response(
            500,
            "unexpected-error",
            "failed to read output stream body while sending image response",
            response_out,
        ) {
            log(
                Level::Error,
                LOG_CONTEXT,
                &format!("failed to send error response: {e}"),
            );
            panic!("failed to send error response");
        }
        return;
    };

    let Ok(stream) = response_body.write() else {
        if let Err(e) = send_error_response(
            500,
            "unexpected-error",
            "failed to get output stream from body while sending image response",
            response_out,
        ) {
            log(
                Level::Error,
                LOG_CONTEXT,
                &format!("failed to send error response: {e}"),
            );
            panic!("failed to send error response");
        }
        return;
    };

    // Write the image bytes to the output stream
    for chunk in image_bytes.chunks(MAX_WRITE_BYTES) {
        if let Err(e) = stream
            .blocking_write_and_flush(chunk)
            .map_err(|e| anyhow!("failed to write chunk: {e}"))
        {
            if let Err(e) = send_error_response(
                500,
                "unexpected-error",
                &format!("failed write image chunk to output stream: {e}"),
                response_out,
            ) {
                log(
                    Level::Error,
                    LOG_CONTEXT,
                    &format!("failed to send error response: {e}"),
                );
                panic!("failed to send error response");
            }
            return;
        }
    }
    ResponseOutparam::set(response_out, Ok(response));
    OutgoingBody::finish(response_body, None).expect("failed to finish response body");
}
