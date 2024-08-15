//! This is a simple component that returns information about any incoming request as JSON
//!
//! So for example, if you send a `GET /index.html`, you should expect to see the following response:
//!
//! ```json,ignore
//! {
//!   "method": "GET",
//!   "path": "/index.html",
//!   "query_string": "",
//!   "body": "",
//! }
//! ```
//!
//! Note that this component has *many* is meant to showcase basic functionality of `wasi:http` --
//! rather than be a completely robust HTTP handler implementation.
//!
//! Some limitations of this component as written:
//!   - Request bodies must fit in component memory (all of it is read in)
//!   - Trailers are not processed/sent
//!
use anyhow::{anyhow, bail, ensure, Result};

wit_bindgen::generate!({
    generate_all
});

/// Implementation of the 'component' world in wit/world.wit will hang off of this struct
struct Component;

use crate::exports::wasi::http::incoming_handler::Guest;
use crate::wasi::http::types::{
    Fields, IncomingBody, IncomingRequest, Method, OutgoingBody, OutgoingResponse, ResponseOutparam,
};
use crate::wasi::io::streams::StreamError;

/// Maximum bytes to read at a time from the incoming request body
/// this value is chosen somewhat arbitrarily, and is not a limit for bytes read,
/// but is instead the amount of bytes to be read *at once*
const MAX_READ_BYTES: u32 = 2048;

/// Maximum bytes to write at a time, due to the limitations on wasi-io's blocking_write_and_flush()
const MAX_WRITE_BYTES: usize = 4096;

impl Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        // Read the path & query from the request
        let path_with_query = request
            .path_with_query()
            .map(String::from)
            .unwrap_or_else(|| "/".into());
        let (path, query) = path_with_query.split_once('?').unwrap_or(("/", ""));
        let method = request.method().to_string();

        // Read the request body bytes into memory
        //
        // NOTE: this implementation cannot handle requests larger than memory,
        // remember that but `wasi:http` is equipped with streams
        // so you can modify this example to work with a request body of *any* size!
        let body_bytes = request
            .read_body()
            .expect("failed to read request body into memory");

        // Build the JSON-ified request body
        let response_json = serde_json::json!({
            "method": method,
            "path": path,
            "query_string": query,
            "body": body_bytes,
        });

        // Convert the JSON object to bytes
        let resp_bytes =
            serde_json::to_vec(&response_json).expect("failed to serialize response body JSON");

        // Build the outgoing response
        let outgoing_response = OutgoingResponse::new(Fields::new());
        outgoing_response
            .send_body(&resp_bytes)
            .expect("failed to send response body");

        // Set the response to be used
        ResponseOutparam::set(response_out, Ok(outgoing_response));
    }
}

// NOTE: Since wit-bindgen makes `IncomingRequest` available to us as a local type,
// we can add convenience functions to it
impl IncomingRequest {
    /// This is a convenience function that writes out the body of a IncomingRequest (from wasi:http)
    /// into anything that supports [`std::io::Write`]
    fn read_body(self) -> Result<Vec<u8>> {
        // Read the body
        let incoming_req_body = self
            .consume()
            .map_err(|()| anyhow!("failed to consume incoming request body"))?;
        let incoming_req_body_stream = incoming_req_body
            .stream()
            .map_err(|()| anyhow!("failed to build stream for incoming request body"))?;
        let mut buf = Vec::<u8>::with_capacity(MAX_READ_BYTES as usize);
        loop {
            match incoming_req_body_stream.read(MAX_READ_BYTES as u64) {
                Ok(bytes) if bytes.is_empty() => break,
                Ok(bytes) => {
                    ensure!(
                        bytes.len() <= MAX_READ_BYTES as usize,
                        "read more bytes than requested"
                    );
                    buf.extend(bytes);
                }
                Err(StreamError::Closed) => break,
                Err(e) => bail!("failed to read bytes: {e}"),
            }
        }
        buf.shrink_to_fit();
        drop(incoming_req_body_stream);
        IncomingBody::finish(incoming_req_body);
        Ok(buf)
    }
}

// NOTE: Since wit-bindgen makes `OutgoingBody` available to us as a local type,
// we can add convenience functions to it
impl OutgoingResponse {
    /// This is a convenience function that writes out the body of a IncomingRequest (from wasi:http)
    /// into anything that supports [`std::io::Read`]
    fn send_body(&self, buf: &[u8]) -> Result<()> {
        let body = self.body().expect("failed to open outgoing response body");
        let out = body
            .write()
            .expect("failed to start writing to outgoing response body");
        for chunk in buf.chunks(MAX_WRITE_BYTES) {
            out.blocking_write_and_flush(chunk)
                .map_err(|e| anyhow!("failed to write chunk: {e}"))?;
        }
        drop(out);
        OutgoingBody::finish(body, None).unwrap();
        Ok(())
    }
}

// NOTE: since wit-bindgen creates these types in our namespace,
// we can hang custom implementations off of them
impl ToString for Method {
    fn to_string(&self) -> String {
        match self {
            Method::Get => "GET".into(),
            Method::Post => "POST".into(),
            Method::Patch => "PATCH".into(),
            Method::Put => "PUT".into(),
            Method::Delete => "DELETE".into(),
            Method::Options => "OPTIONS".into(),
            Method::Head => "HEAD".into(),
            Method::Connect => "CONNECT".into(),
            Method::Trace => "TRACE".into(),
            Method::Other(m) => m.into(),
        }
    }
}

export!(Component);
