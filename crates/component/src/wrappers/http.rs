//! This module provides utilities for writing HTTP servers and clients using the WASI HTTP API.
//!
//! It's inspired by the WASI 0.3 proposal for <https://github.com/WebAssembly/wasi-http> and will
//! be supported until the release of wasi:http@0.3.0. After that, this module will be deprecated.
//!
//! This crate can be used with the [reqwest-wasmcloud](https://github.com/wasmCloud/reqwest-wasmcloud)
//! crate to make outgoing HTTP requests from a WebAssembly component.
//!
//! ```rust
//! use wasi::http::types::ErrorCode;
//! use wasmcloud_component::http::{HttpServer, Request, Response};
//!
//! struct Component;
//!
//! wasmcloud_component::http::export!(Component);
//!
//! // Implementing the [`HttpServer`] trait for a component
//! impl HttpServer for Component {
//!     fn handle(_request: Request) -> Result<Response, ErrorCode> {
//!         Ok(Response::ok("Hello from Rust!".into()))
//!     }
//! }
//! ```
// Required until <https://github.com/seanmonstar/reqwest/pull/2453> merges
use reqwest_wasmcloud as reqwest;

use std::{
    io::{Read, Write},
    ops::Deref,
    str::FromStr,
};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use wasi::{
    http::{
        outgoing_handler::ErrorCode,
        types::{
            Fields, IncomingBody, Method, OutgoingBody, OutgoingResponse, ResponseOutparam, Scheme,
        },
    },
    io::streams::{InputStream, StreamError},
};

/// Trait for implementing an HTTP server WebAssembly component that receives a
/// [`Request`] and returns a [`ResponseBuilder`].
pub trait HttpServer {
    fn handle(request: Request) -> Result<Response, ErrorCode>;
}

/// Wrapper for incoming requests from the WASI HTTP API. This type includes methods
/// to access information on the inner [`wasi::http::types::IncomingRequest`], and assists
/// with properly handling the request body.
///
/// This can be converted into the inner [`wasi::http::types::IncomingRequest`] using
/// the `Into` impl, or into a [`reqwest::Request`] using [`TryInto<reqwest::Request>`].
pub struct Request {
    inner: wasi::http::types::IncomingRequest,
}

impl From<wasi::http::types::IncomingRequest> for Request {
    fn from(inner: wasi::http::types::IncomingRequest) -> Self {
        Self { inner }
    }
}

impl Request {
    /// Read the entire body of the [`Request`] into a buffer and return it.
    /// This consumes the request and finishes the body.
    ///
    /// For very large bodies, consider using [`Request::into_body_stream`] instead.
    pub fn into_body(self) -> Result<Vec<u8>, ErrorCode> {
        let (mut input_stream, incoming_body) = self.consume_request()?;
        let mut buf = vec![];
        input_stream
            .read_to_end(&mut buf)
            .map_err(|_| error_code("failed to read incoming body"))?;
        drop(input_stream);
        IncomingBody::finish(incoming_body);
        Ok(buf)
    }

    /// Converts the request into the inner [`InputStream`] and [`IncomingBody`]
    ///
    /// These values have a tightly coupled lifecycle and the stream must be dropped
    /// before the body is finished.
    ///
    /// Use this in tandem with [`ResponseBuilder::new_stream`] to return a response with a body stream.
    pub fn into_body_stream(self) -> Result<(InputStream, IncomingBody), ErrorCode> {
        self.consume_request()
    }

    /// Consume the request and return the inner input stream and incoming body.
    fn consume_request(self) -> Result<(InputStream, IncomingBody), ErrorCode> {
        let incoming_body = self
            .inner
            .consume()
            .map_err(|_| error_code("failed to consume incoming request"))?;
        let input_stream = incoming_body
            .stream()
            .map_err(|_| error_code("failed to get incoming body stream from incoming request"))?;
        Ok((input_stream, incoming_body))
    }
}

/// Allows for calling methods on the inner [`wasi::http::types::IncomingRequest`] directly.
impl Deref for Request {
    type Target = wasi::http::types::IncomingRequest;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Convert a [`Request`] into the inner [`wasi::http::types::IncomingRequest`].
impl From<Request> for wasi::http::types::IncomingRequest {
    fn from(request: Request) -> Self {
        request.inner
    }
}

impl TryInto<reqwest::Request> for Request {
    type Error = ErrorCode;

    fn try_into(self) -> Result<reqwest::Request, Self::Error> {
        let mut headers = HeaderMap::new();
        for (name, value) in self.headers().entries() {
            headers.append(
                HeaderName::from_str(&name).map_err(error_code)?,
                HeaderValue::from_bytes(&value).map_err(error_code)?,
            );
        }
        let method = match self.method() {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Delete => reqwest::Method::DELETE,
            Method::Head => reqwest::Method::HEAD,
            Method::Options => reqwest::Method::OPTIONS,
            Method::Connect => reqwest::Method::CONNECT,
            Method::Patch => reqwest::Method::PATCH,
            Method::Trace => reqwest::Method::TRACE,
            Method::Other(s) => reqwest::Method::from_bytes(s.as_bytes()).map_err(|_| {
                error_code(format!("failed to convert method {s} to reqwest::Method"))
            })?,
        };

        let scheme = match self.scheme() {
            Some(Scheme::Http) => http::uri::Scheme::HTTP.to_string(),
            Some(Scheme::Https) => http::uri::Scheme::HTTPS.to_string(),
            Some(Scheme::Other(s)) => s,
            None => return Err(error_code("missing scheme in incoming request")),
        };
        let authority = match self.authority() {
            Some(authority) => authority,
            None => return Err(error_code("missing authority in incoming request")),
        };
        let path_with_query = self.path_with_query().unwrap_or_default();
        let url = reqwest::Url::from_str(&format!("{}://{}{}", scheme, authority, path_with_query))
            .map_err(error_code)?;

        // Using reqwest::Request instead of reqwest::RequestBuilder to avoid constructing
        // a reqwest::Client for each request.
        let mut req = reqwest::Request::new(method, url);
        req.headers_mut().extend(headers);
        req.body_mut().replace(
            self.into_body()
                .map_err(|_| error_code("failed to read incoming body"))?
                .into(),
        );

        Ok(req)
    }
}

// wasi:http/incoming-handler utilities and wrappers

/// Builder for constructing an HTTP response to be returned from an [`HttpServer`].
///
/// This type is used to construct a response with a status code, body, and headers.
/// The response body can either be a byte slice or a [`wasi::io::streams::InputStream`].
pub struct ResponseBuilder {
    pub(crate) status_code: u16,
    pub(crate) body: Option<Vec<u8>>,
    pub(crate) body_stream: Option<(
        wasi::io::streams::InputStream,
        Option<wasi::http::types::IncomingBody>,
    )>,
    pub(crate) headers: HeaderMap,
    // TODO(followup): Add trailers
}

impl ResponseBuilder {
    /// Return a new [`ResponseBuilder`] with the provided status code and body
    pub fn new(status_code: u16, body: Vec<u8>) -> Self {
        Self {
            status_code,
            body: Some(body),
            body_stream: None,
            headers: HeaderMap::new(),
        }
    }

    /// Return a new `ResponseBuilder` with the provided status code and body stream.
    ///
    /// If the provided stream is from an incoming HTTP body (either a request body or a response body),
    /// you should provide the handle to the incoming body as well. This will ensure that the incoming
    /// body is properly finished when the response is finished.
    ///
    /// ```rust
    /// // Example that streams the incoming body to the outgoing body
    /// use wasi::http::types::ErrorCode;
    /// use wasmcloud_component::http::{HttpServer, Request, Response, ResponseBuilder};
    /// struct Component;
    /// wasmcloud_component::http::export!(Component);
    ///
    /// impl HttpServer for Component {
    ///     fn handle(request: Request) -> Result<Response, ErrorCode> {
    ///         let (stream, body) = request.into_body_stream()?;
    ///         Ok(ResponseBuilder::new_stream(200, stream, Some(body)).build())
    ///     }
    /// }
    /// ```
    ///
    /// ```rust
    /// // Example that streams a response body to the client
    /// use wasi::http::types::ErrorCode;
    /// use wasmcloud_component::http::{HttpServer, Request, Response, ResponseBuilder};
    /// use reqwest_wasmcloud as reqwest;
    ///
    /// struct Component;
    /// wasmcloud_component::http::export!(Component);
    ///
    /// impl HttpServer for Component {
    ///     fn handle(_request: Request) -> Result<Response, ErrorCode> {
    ///         let mut response: reqwest::Response = reqwest::get("https://example.com").map_err(|e| {
    ///             ErrorCode::InternalError(Some(format!("failed to send outbound request {e:?}")))
    ///         })?;
    ///         let (stream, body) = response.bytes_stream().map_err(|e| {
    ///             ErrorCode::InternalError(Some(format!("failed to read response body {e:?}")))
    ///         })?;
    ///         Ok(ResponseBuilder::new_stream(200, stream, Some(body)).build())
    ///     }
    /// }
    /// ```
    pub fn new_stream(
        status_code: u16,
        stream: wasi::io::streams::InputStream,
        body: Option<wasi::http::types::IncomingBody>,
    ) -> Self {
        Self {
            status_code,
            body: None,
            body_stream: Some((stream, body)),
            headers: HeaderMap::new(),
        }
    }

    pub fn status_code(mut self, status_code: u16) -> Self {
        self.status_code = status_code;
        self
    }

    pub fn headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    pub fn build(self) -> Response {
        Response {
            status_code: self.status_code,
            headers: self.headers,
            body: self.body,
            body_stream: self.body_stream,
        }
    }
}

/// An HTTP response to be returned from an [`HttpServer`] trait implementation.
///
/// Unless using a helper function like [`Response::ok`], you should construct this
/// using a [`ResponseBuilder`].
pub struct Response {
    pub(crate) status_code: u16,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Option<Vec<u8>>,
    pub(crate) body_stream: Option<(InputStream, Option<IncomingBody>)>,
}

impl Response {
    /// Helper function to return an OK response with a body.
    pub fn ok(body: Vec<u8>) -> Self {
        Self {
            status_code: 200,
            headers: HeaderMap::new(),
            body: Some(body),
            body_stream: None,
        }
    }
}

// Macro wrapper for wasi:http/incoming-handler

/// Macro to export [`wasi::exports::http::incoming_handler::Guest`] implementation for a type that
/// implements [`HttpServer`]. This aims to be as similar as possible to [`wasi::http::proxy::export!`].
///
/// While the code wasn't copied and this macro is different, the nice experience of the macro to wrap
/// the Guest implementation in <https://github.com/wacker-dev/waki/blob/main/waki-macros/src/export.rs>
/// inspired this macro.
#[macro_export]
macro_rules! export {
    ($t:ty) => {
        impl ::wasi::exports::http::incoming_handler::Guest for $t {
            fn handle(
                incoming_request: ::wasi::http::types::IncomingRequest,
                response_outparam: ::wasi::http::types::ResponseOutparam,
            ) {
                match <$t as ::wasmcloud_component::http::HttpServer>::handle(
                    incoming_request.into(),
                ) {
                    Ok(response) => ::wasmcloud_component::http::set_outgoing_response(
                        response,
                        response_outparam,
                    ),
                    Err(error) => {
                        ::wasi::http::types::ResponseOutparam::set(response_outparam, Err(error))
                    }
                }
            }
        }
        type ComponentHttpExportAlias = $t;
        ::wasi::http::proxy::export!(ComponentHttpExportAlias);
    };
}
pub use export;

/// Convert a  [`ResponseBuilder`] into an [`wasi::http::types::OutgoingResponse`] and set that response.
///
/// This is primarily public for the [`export`] macro and should only be used directly with care.
#[doc(hidden)]
pub fn set_outgoing_response(user_response: Response, response_out: ResponseOutparam) {
    // Construct response, returning server errors if possible
    let headers = Fields::new();
    user_response.headers.into_iter().for_each(|(name, value)| {
        headers
            .set(&name.unwrap().to_string(), &[value.as_bytes().to_vec()])
            .expect("failed to set header");
    });
    let response = OutgoingResponse::new(headers);
    if response.set_status_code(user_response.status_code).is_err() {
        ResponseOutparam::set(response_out, Err(error_code("failed to set status code")));
        return;
    }

    match (user_response.body, user_response.body_stream) {
        (Some(body), None) => {
            let Ok(response_body) = response.body() else {
                ResponseOutparam::set(
                    response_out,
                    Err(error_code("failed to get outgoing body handle")),
                );
                return;
            };
            let Ok(mut response_write) = response_body.write() else {
                ResponseOutparam::set(
                    response_out,
                    Err(error_code("failed to get write handle to outgoing body")),
                );
                return;
            };

            // Set the response before writing the body. At this point, an error can't be returned.
            ResponseOutparam::set(response_out, Ok(response));
            response_write
                .write_all(&body)
                .expect("failed to write body stream");
            drop(response_write);
            OutgoingBody::finish(response_body, None).expect("failed to finish outgoing body");
        }
        (None, Some((stream, body))) => {
            let Ok(response_body) = response.body() else {
                ResponseOutparam::set(
                    response_out,
                    Err(error_code("failed to get outgoing body handle")),
                );
                return;
            };
            let Ok(response_write) = response_body.write() else {
                ResponseOutparam::set(
                    response_out,
                    Err(error_code("failed to get write handle to outgoing body")),
                );
                return;
            };

            // Set the response before writing the body. At this point, an error can't be returned.
            ResponseOutparam::set(response_out, Ok(response));
            loop {
                match response_write.blocking_splice(&stream, u64::MAX) {
                    Ok(0) | Err(StreamError::Closed) => break,
                    Ok(_) => continue,
                    Err(StreamError::LastOperationFailed(_)) => {
                        // TODO: log error
                        return;
                    }
                }
            }
            // Drop the input stream, finish the incoming body
            drop(stream);
            if let Some(body) = body {
                IncomingBody::finish(body);
            }
            // Drop the output stream, finish the outgoing body
            drop(response_write);
            OutgoingBody::finish(response_body, None).expect("failed to finish outgoing body");
        }
        (Some(_), Some(_)) => {
            ResponseOutparam::set(
                response_out,
                Err(error_code(
                    "cannot set both body and body stream in response",
                )),
            );
        }
        (None, None) => ResponseOutparam::set(response_out, Ok(response)),
    }
}

/// Helper function to construct an internal server `ErrorCode` with an error message.
fn error_code(e: impl ToString) -> ErrorCode {
    ErrorCode::InternalError(Some(e.to_string()))
}
