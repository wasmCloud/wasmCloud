#![allow(clippy::missing_safety_doc)]
wit_bindgen::generate!({
    world: "component"
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

impl Guest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        // Pull together information from the request
        let method: String = match request.method() {
            Method::Get => "GET".into(),
            Method::Post => "POST".into(),
            Method::Patch => "PATCH".into(),
            Method::Put => "PUT".into(),
            Method::Delete => "DELETE".into(),
            Method::Options => "OPTIONS".into(),
            Method::Head => "HEAD".into(),
            Method::Connect => "CONNECT".into(),
            Method::Trace => "TRACE".into(),
            Method::Other(m) => m,
        };

        // Read the body
        let Ok(incoming_req_body) = request.consume() else {
            panic!("failed to consume incoming request body");
        };
        let Ok(incoming_req_body_stream) = incoming_req_body.stream() else {
            panic!("failed to build stream from incoming request body");
        };
        let mut body_buf = Vec::<u8>::with_capacity(MAX_READ_BYTES as usize);
        loop {
            match incoming_req_body_stream.read(MAX_READ_BYTES as u64) {
                Ok(bytes) => body_buf.extend(bytes),
                Err(StreamError::Closed) => {
                    body_buf.shrink_to_fit();
                    break;
                }
                Err(e) => {
                    panic!("failed to read bytes: {e}");
                }
            }
        }
        IncomingBody::finish(incoming_req_body);

        // Build the information that will be returned
        let path_with_query = request
            .path_with_query()
            .map(String::from)
            .unwrap_or_else(|| "/".into());
        let (path, query) = path_with_query.split_once('?').unwrap_or(("/", ""));
        let response_json = serde_json::json!({
                    "method": method,
                    "path": path,
                    "query_string": query,
                    "body": body_buf,
        });

        let Ok(resp_bytes) = serde_json::to_vec(&response_json) else {
            panic!("failed to serialize response body JSON");
        };

        // Build the outgoing response
        let resp = OutgoingResponse::new(Fields::new());
        let body = resp.body().expect("failed to open outgoing response body");
        let out = body
            .write()
            .expect("failed to start writing to outgoing response body");
        // Since blocking_write_and_flush only writes up to 4096 bytes, we must
        // chunk the bytes to be written, and then write them to the output stream
        for chunk in resp_bytes.chunks(4096) {
            if let Err(e) = out.blocking_write_and_flush(chunk) {
                panic!("failed to perform blocking write of chunk: {e}");
            }
        }
        drop(out);
        OutgoingBody::finish(body, None).unwrap();

        // Set the response to be used
        ResponseOutparam::set(outparam, Ok(resp));
    }
}

export!(Component);
