//! This module provides utilities for writing HTTP servers and clients using the WASI HTTP API.
//!
//! It's inspired by the WASI 0.3 proposal for <https://github.com/WebAssembly/wasi-http> and will
//! be supported until the release of wasi:http@0.3.0. After that, this module will likely be deprecated.
//!
//! ```rust
//! use wasmcloud_component::http;
//!
//! struct Component;
//!
//! http::export!(Component);
//!
//! // Implementing the [`Server`] trait for a component
//! impl http::Server for Component {
//!     fn handle(_request: http::IncomingRequest) -> http::Result<http::Response<impl http::OutgoingBody>> {
//!         Ok(http::Response::new("Hello from Rust!"))
//!     }
//! }
//! ```
use core::fmt::Display;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

use std::io::{Read, Write};

use anyhow::{anyhow, Context as _};
use wasi::http::types::{OutgoingResponse, ResponseOutparam};
use wasi::io::streams::{InputStream, OutputStream, StreamError};

pub use http::{
    header, method, response, uri, HeaderMap, HeaderName, HeaderValue, Method, Request, Response,
    StatusCode, Uri,
};
pub use wasi::http::types::ErrorCode;

pub type Result<T, E = ErrorCode> = core::result::Result<T, E>;

pub type IncomingRequest = Request<IncomingBody>;

impl crate::From<Method> for wasi::http::types::Method {
    fn from(method: Method) -> Self {
        match method.as_str() {
            "GET" => Self::Get,
            "HEAD" => Self::Head,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "DELETE" => Self::Delete,
            "CONNECT" => Self::Connect,
            "OPTIONS" => Self::Options,
            "TRACE" => Self::Trace,
            "PATCH" => Self::Patch,
            _ => Self::Other(method.to_string()),
        }
    }
}

impl crate::TryFrom<wasi::http::types::Method> for Method {
    type Error = method::InvalidMethod;

    fn try_from(method: wasi::http::types::Method) -> Result<Self, Self::Error> {
        match method {
            wasi::http::types::Method::Get => Ok(Self::GET),
            wasi::http::types::Method::Head => Ok(Self::HEAD),
            wasi::http::types::Method::Post => Ok(Self::POST),
            wasi::http::types::Method::Put => Ok(Self::PUT),
            wasi::http::types::Method::Delete => Ok(Self::DELETE),
            wasi::http::types::Method::Connect => Ok(Self::CONNECT),
            wasi::http::types::Method::Options => Ok(Self::OPTIONS),
            wasi::http::types::Method::Trace => Ok(Self::TRACE),
            wasi::http::types::Method::Patch => Ok(Self::PATCH),
            wasi::http::types::Method::Other(method) => method.parse(),
        }
    }
}

impl crate::From<uri::Scheme> for wasi::http::types::Scheme {
    fn from(scheme: uri::Scheme) -> Self {
        match scheme.as_str() {
            "http" => Self::Http,
            "https" => Self::Https,
            _ => Self::Other(scheme.to_string()),
        }
    }
}

impl crate::TryFrom<wasi::http::types::Scheme> for http::uri::Scheme {
    type Error = uri::InvalidUri;

    fn try_from(scheme: wasi::http::types::Scheme) -> Result<Self, Self::Error> {
        match scheme {
            wasi::http::types::Scheme::Http => Ok(Self::HTTP),
            wasi::http::types::Scheme::Https => Ok(Self::HTTPS),
            wasi::http::types::Scheme::Other(scheme) => scheme.parse(),
        }
    }
}

#[derive(Debug)]
pub enum FieldsToHeaderMapError {
    InvalidHeaderName(header::InvalidHeaderName),
    InvalidHeaderValue(header::InvalidHeaderValue),
}

impl Display for FieldsToHeaderMapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FieldsToHeaderMapError::InvalidHeaderName(e) => write!(f, "invalid header name: {e}"),
            FieldsToHeaderMapError::InvalidHeaderValue(e) => write!(f, "invalid header value: {e}"),
        }
    }
}

impl std::error::Error for FieldsToHeaderMapError {}

impl crate::TryFrom<wasi::http::types::Fields> for HeaderMap {
    type Error = FieldsToHeaderMapError;

    fn try_from(fields: wasi::http::types::Fields) -> Result<Self, Self::Error> {
        let mut headers = HeaderMap::new();
        for (name, value) in fields.entries() {
            let name =
                HeaderName::try_from(name).map_err(FieldsToHeaderMapError::InvalidHeaderName)?;
            let value =
                HeaderValue::try_from(value).map_err(FieldsToHeaderMapError::InvalidHeaderValue)?;
            match headers.entry(name) {
                header::Entry::Vacant(entry) => {
                    entry.insert(value);
                }
                header::Entry::Occupied(mut entry) => {
                    entry.append(value);
                }
            };
        }
        Ok(headers)
    }
}

impl crate::TryFrom<HeaderMap> for wasi::http::types::Fields {
    type Error = wasi::http::types::HeaderError;

    fn try_from(headers: HeaderMap) -> Result<Self, Self::Error> {
        let fields = wasi::http::types::Fields::new();
        for (name, value) in &headers {
            fields.append(name.as_ref(), value.as_bytes())?;
        }
        Ok(fields)
    }
}

/// Trait for implementing a type that can be written to an outgoing HTTP response body.
///
/// When implementing this trait, you should write your type to the provided `OutputStream` and then
/// drop the stream. Finally, you should call `wasi::http::types::OutgoingBody::finish` to finish
/// the body and return the result.
///
/// This trait is already implemented for common Rust types, and it's implemented for
/// `wasi::http::types::IncomingBody` and `wasi::io::streams::InputStream` as well. This enables
/// using any stream from a Wasm interface as an outgoing body.
///
/// ```ignore
/// use std::io::Write;
///
/// impl wasmcloud_component::http::OutgoingBody for Vec<u8> {
///    fn write(
///        self,
///        body: wasi::http::types::OutgoingBody,
///        mut stream: wasi::io::streams::OutputStream,
///    ) -> std::io::Result<()> {
///        stream.write_all(&self)?;
///        drop(stream);
///        wasi::http::types::OutgoingBody::finish(body, None)
///            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
///     }
/// }
/// ```
pub trait OutgoingBody {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        stream: OutputStream,
    ) -> std::io::Result<()>;
}

impl OutgoingBody for () {
    fn write(
        self,
        _body: wasi::http::types::OutgoingBody,
        _stream: OutputStream,
    ) -> std::io::Result<()> {
        Ok(())
    }
}

impl OutgoingBody for &[u8] {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        mut stream: OutputStream,
    ) -> std::io::Result<()> {
        stream.write_all(self)?;
        drop(stream);
        wasi::http::types::OutgoingBody::finish(body, None).map_err(std::io::Error::other)
    }
}

impl OutgoingBody for Box<[u8]> {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        stream: OutputStream,
    ) -> std::io::Result<()> {
        self.as_ref().write(body, stream)
    }
}

impl OutgoingBody for Vec<u8> {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        stream: OutputStream,
    ) -> std::io::Result<()> {
        self.as_slice().write(body, stream)
    }
}

impl OutgoingBody for &str {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        stream: OutputStream,
    ) -> std::io::Result<()> {
        self.as_bytes().write(body, stream)
    }
}

impl OutgoingBody for Box<str> {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        stream: OutputStream,
    ) -> std::io::Result<()> {
        self.as_ref().write(body, stream)
    }
}

impl OutgoingBody for String {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        stream: OutputStream,
    ) -> std::io::Result<()> {
        self.as_str().write(body, stream)
    }
}

impl OutgoingBody for InputStream {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        stream: OutputStream,
    ) -> std::io::Result<()> {
        loop {
            match stream.blocking_splice(&self, u64::MAX) {
                Ok(0) | Err(StreamError::Closed) => break,
                Ok(_) => continue,
                Err(StreamError::LastOperationFailed(err)) => {
                    return Err(std::io::Error::other(err));
                }
            }
        }
        drop(stream);
        wasi::http::types::OutgoingBody::finish(body, None).map_err(std::io::Error::other)
    }
}

impl OutgoingBody for wasi::http::types::IncomingBody {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        stream: OutputStream,
    ) -> std::io::Result<()> {
        let input = self
            .stream()
            .map_err(|()| std::io::Error::from(std::io::ErrorKind::Other))?;
        loop {
            match stream.blocking_splice(&input, u64::MAX) {
                Ok(0) | Err(StreamError::Closed) => break,
                Ok(_) => continue,
                Err(StreamError::LastOperationFailed(err)) => {
                    return Err(std::io::Error::other(err));
                }
            }
        }
        drop(stream);
        let _trailers = wasi::http::types::IncomingBody::finish(self);
        // NOTE: getting trailers crashes Wasmtime 25, so avoid doing so
        //let trailers = if let Some(trailers) = trailers.get() {
        //    trailers
        //} else {
        //    trailers.subscribe().block();
        //    trailers
        //        .get()
        //        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::Other))?
        //};
        //let trailers = trailers.map_err(|()| std::io::Error::from(std::io::ErrorKind::Other))?;
        //let trailers =
        //    trailers.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
        wasi::http::types::OutgoingBody::finish(body, None).map_err(std::io::Error::other)
    }
}

impl OutgoingBody for IncomingBody {
    fn write(
        self,
        body: wasi::http::types::OutgoingBody,
        stream: OutputStream,
    ) -> std::io::Result<()> {
        loop {
            match stream.blocking_splice(&self.stream, u64::MAX) {
                Ok(0) | Err(StreamError::Closed) => break,
                Ok(_) => continue,
                Err(StreamError::LastOperationFailed(err)) => {
                    return Err(std::io::Error::other(err));
                }
            }
        }
        drop(stream);
        let trailers = self.into_trailers_wasi().map_err(std::io::Error::other)?;
        wasi::http::types::OutgoingBody::finish(body, trailers).map_err(std::io::Error::other)
    }
}

/// Wraps a body, which implements [Read]
#[derive(Clone, Copy, Debug)]
pub struct ReadBody<T>(T);

impl<T: Read> OutgoingBody for ReadBody<T> {
    fn write(
        mut self,
        body: wasi::http::types::OutgoingBody,
        mut stream: OutputStream,
    ) -> std::io::Result<()> {
        std::io::copy(&mut self.0, &mut stream)?;
        drop(stream);
        wasi::http::types::OutgoingBody::finish(body, None).map_err(std::io::Error::other)
    }
}

/// Wraps the incoming body of a request which just contains
/// the stream and the body of the request itself. The bytes of the body
/// are only read into memory explicitly. The implementation of [`OutgoingBody`]
/// for this type will read the bytes from the stream and write them to the
/// output stream.
pub struct IncomingBody {
    stream: InputStream,
    body: wasi::http::types::IncomingBody,
}

impl TryFrom<wasi::http::types::IncomingBody> for IncomingBody {
    type Error = anyhow::Error;

    fn try_from(body: wasi::http::types::IncomingBody) -> Result<Self, Self::Error> {
        let stream = body
            .stream()
            .map_err(|()| anyhow!("failed to get incoming request body"))?;
        Ok(Self { body, stream })
    }
}

impl TryFrom<wasi::http::types::IncomingRequest> for IncomingBody {
    type Error = anyhow::Error;

    fn try_from(request: wasi::http::types::IncomingRequest) -> Result<Self, Self::Error> {
        let body = request
            .consume()
            .map_err(|()| anyhow!("failed to consume incoming request"))?;
        body.try_into()
    }
}

impl Deref for IncomingBody {
    type Target = InputStream;

    fn deref(&self) -> &Self::Target {
        &self.stream
    }
}

impl DerefMut for IncomingBody {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stream
    }
}

impl Read for IncomingBody {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        Read::read(&mut self.stream, buf)
    }
}

impl IncomingBody {
    pub fn into_trailers(self) -> anyhow::Result<Option<HeaderMap>> {
        let trailers = self.into_trailers_wasi()?;
        let trailers = trailers.map(crate::TryInto::try_into).transpose()?;
        Ok(trailers)
    }

    pub fn into_trailers_wasi(self) -> anyhow::Result<Option<wasi::http::types::Fields>> {
        let IncomingBody { body, stream } = self;
        drop(stream);
        let _trailers = wasi::http::types::IncomingBody::finish(body);
        // NOTE: getting trailers crashes Wasmtime 25, so avoid doing so
        //let trailers = if let Some(trailers) = trailers.get() {
        //    trailers
        //} else {
        //    trailers.subscribe().block();
        //    trailers.get().context("trailers missing")?
        //};
        //let trailers = trailers.map_err(|()| anyhow!("trailers already consumed"))?;
        //trailers.context("failed to receive trailers")
        Ok(None)
    }
}

impl crate::TryFrom<wasi::http::types::IncomingRequest> for Request<IncomingBody> {
    type Error = anyhow::Error;

    fn try_from(request: wasi::http::types::IncomingRequest) -> Result<Self, Self::Error> {
        let uri = Uri::builder();
        let uri = if let Some(path_with_query) = request.path_with_query() {
            uri.path_and_query(path_with_query)
        } else {
            uri.path_and_query("/")
        };
        let uri = if let Some(scheme) = request.scheme() {
            let scheme = <uri::Scheme as crate::TryFrom<_>>::try_from(scheme)
                .context("failed to convert scheme")?;
            uri.scheme(scheme)
        } else {
            uri
        };
        let uri = if let Some(authority) = request.authority() {
            uri.authority(authority)
        } else {
            uri
        };
        let uri = uri.build().context("failed to build URI")?;
        let method = <Method as crate::TryFrom<_>>::try_from(request.method())
            .context("failed to convert method")?;
        let mut req = Request::builder().method(method).uri(uri);
        let req_headers = req
            .headers_mut()
            .context("failed to construct header map")?;
        *req_headers = crate::TryInto::try_into(request.headers())
            .context("failed to convert header fields to header map")?;
        let body = IncomingBody::try_from(request)?;
        req.body(body).context("failed to construct request")
    }
}

#[doc(hidden)]
#[derive(Default, Debug, Copy, Clone)]
pub struct IncomingHandler<T: ?Sized>(PhantomData<T>);

pub enum ResponseError {
    /// Status code is not valid
    StatusCode(StatusCode),
    /// Failed to set headers
    Headers(wasi::http::types::HeaderError),
    /// Failed to get outgoing response body
    Body,
    /// Failed to get outgoing response body stream
    BodyStream,
}

/// Trait for implementing an HTTP server WebAssembly component that receives a
/// [`IncomingRequest`] and returns a [`Response`].
pub trait Server {
    fn handle(request: IncomingRequest) -> Result<Response<impl OutgoingBody>, ErrorCode>;

    fn request_error(err: anyhow::Error) {
        eprintln!("failed to convert `wasi:http/types.incoming-request` to `http::Request`: {err}");
    }

    fn response_error(out: ResponseOutparam, err: ResponseError) {
        match err {
            ResponseError::StatusCode(code) => {
                ResponseOutparam::set(
                    out,
                    Err(ErrorCode::InternalError(Some(format!(
                        "code `{code}` is not a valid HTTP status code",
                    )))),
                );
            }
            ResponseError::Headers(err) => {
                ResponseOutparam::set(
                    out,
                    Err(ErrorCode::InternalError(Some(format!(
                        "{:#}",
                        anyhow!(err).context("failed to set headers"),
                    )))),
                );
            }
            ResponseError::Body => {
                ResponseOutparam::set(
                    out,
                    Err(ErrorCode::InternalError(Some(
                        "failed to get response body".into(),
                    ))),
                );
            }
            ResponseError::BodyStream => {
                ResponseOutparam::set(
                    out,
                    Err(ErrorCode::InternalError(Some(
                        "failed to get response body stream".into(),
                    ))),
                );
            }
        }
    }

    fn body_error(err: std::io::Error) {
        eprintln!("failed to write response body: {err}");
    }
}

impl<T: Server + ?Sized> wasi::exports::http::incoming_handler::Guest for IncomingHandler<T> {
    fn handle(
        request: wasi::http::types::IncomingRequest,
        response_out: wasi::http::types::ResponseOutparam,
    ) {
        match crate::TryInto::try_into(request) {
            Ok(request) => match T::handle(request) {
                Ok(response) => {
                    let (
                        response::Parts {
                            status, headers, ..
                        },
                        body,
                    ) = response.into_parts();

                    let headers = match crate::TryInto::try_into(headers) {
                        Ok(headers) => headers,
                        Err(err) => {
                            T::response_error(response_out, ResponseError::Headers(err));
                            return;
                        }
                    };
                    let resp_tx = OutgoingResponse::new(headers);
                    if let Err(()) = resp_tx.set_status_code(status.as_u16()) {
                        T::response_error(response_out, ResponseError::StatusCode(status));
                        return;
                    }

                    let Ok(resp_body) = resp_tx.body() else {
                        T::response_error(response_out, ResponseError::Body);
                        return;
                    };

                    let Ok(stream) = resp_body.write() else {
                        T::response_error(response_out, ResponseError::BodyStream);
                        return;
                    };

                    ResponseOutparam::set(response_out, Ok(resp_tx));
                    if let Err(err) = body.write(resp_body, stream) {
                        T::body_error(err);
                    }
                }
                Err(err) => ResponseOutparam::set(response_out, Err(err)),
            },
            Err(err) => T::request_error(err),
        }
    }
}

// Macro wrapper for wasi:http/incoming-handler

/// Macro to export [`wasi::exports::http::incoming_handler::Guest`] implementation for a type that
/// implements [`Server`]. This aims to be as similar as possible to [`wasi::http::proxy::export!`].
#[macro_export]
macro_rules! export {
    ($t:ty) => {
        type __IncomingHandlerExport = ::wasmcloud_component::http::IncomingHandler<$t>;
        ::wasmcloud_component::wasi::http::proxy::export!(__IncomingHandlerExport with_types_in ::wasmcloud_component::wasi);
    };
}
pub use export;
