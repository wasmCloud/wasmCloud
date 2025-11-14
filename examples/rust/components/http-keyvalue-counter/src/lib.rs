mod bindings {
    use crate::Component;

    wit_bindgen::generate!({ generate_all });

    export!(Component);
}

use bindings::wasi::http::types::*;
use bindings::wasi::keyvalue::atomics;
use bindings::wasi::keyvalue::store;

struct Component;

impl bindings::exports::wasi::http::incoming_handler::Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        // Parse the request to get the path
        let path = get_path_from_request(&_request).unwrap_or_else(|| "/default".to_string());

        // Use the path as the counter key (removing leading slash)
        let key = path.trim_start_matches('/');
        let key = if key.is_empty() { "default" } else { key };

        // Open the default keyvalue store bucket
        let bucket = match store::open("default") {
            Ok(bucket) => bucket,
            Err(e) => {
                send_error_response(
                    response_out,
                    500,
                    &format!("Failed to open bucket: {}", e),
                );
                return;
            }
        };

        // Increment the counter
        let count = match atomics::increment(&bucket, key, 1) {
            Ok(count) => count,
            Err(e) => {
                send_error_response(
                    response_out,
                    500,
                    &format!("Failed to increment counter: {}", e),
                );
                return;
            }
        };

        // Build and send response
        let response = OutgoingResponse::new(Fields::new());
        let response_body = response.body().expect("response body to exist");
        let stream = response_body
            .write()
            .expect("failed to get output stream");
        ResponseOutparam::set(response_out, Ok(response));

        let body = format!("Counter {}: {}\n", key, count);
        stream
            .blocking_write_and_flush(body.as_bytes())
            .expect("failed to write response");

        drop(stream);
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
    }
}

fn get_path_from_request(request: &IncomingRequest) -> Option<String> {
    let headers = request.headers();
    let entries = headers.entries();

    for (name, value) in entries {
        if name.to_lowercase() == ":path" {
            return String::from_utf8(value).ok();
        }
    }

    // Fallback: try to get path from authority header or use default
    None
}

fn send_error_response(response_out: ResponseOutparam, status: u16, message: &str) {
    let response = OutgoingResponse::new(Fields::new());
    response.set_status_code(status).ok();
    let response_body = response.body().expect("response body to exist");
    let stream = response_body
        .write()
        .expect("failed to get output stream");
    ResponseOutparam::set(response_out, Ok(response));

    stream
        .blocking_write_and_flush(message.as_bytes())
        .ok();

    drop(stream);
    OutgoingBody::finish(response_body, None).ok();
}
