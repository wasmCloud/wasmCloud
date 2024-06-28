#![allow(clippy::missing_safety_doc)]
wit_bindgen::generate!();

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::*;

struct KeyvalueCounterAuth;

impl Guest for KeyvalueCounterAuth {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        // Get the path & query to the incoming request
        let path_with_query = request
            .path_with_query()
            .expect("failed to get path with query");

        // At first, we can assume the object name will be the path with query
        // (ex. simple paths like '/some-key-here')
        let mut object_name = path_with_query.clone();

        // If query parameters were supplied, then we need to recalculate the object_name
        // and take special actions if some parameters (like `link_name`) are present (and ignore the rest)
        if let Some((path, _query)) = path_with_query.split_once('?') {
            object_name = path.to_string();
        }

        if !user_is_authorized(&request) {
            let response = OutgoingResponse::new(Fields::new());
            response.set_status_code(401).unwrap();
            let response_body = response.body().unwrap();
            response_body
                .write()
                .unwrap()
                .blocking_write_and_flush("Unauthorized\n".as_bytes())
                .unwrap();
            OutgoingBody::finish(response_body, None).expect("failed to finish response body");
            ResponseOutparam::set(response_out, Ok(response));
            return;
        }

        // Note that the keyvalue-redis provider used with this example creates a single global bucket.
        // While the wasi-keyvalue interface supports multiple named buckets, the keyvalue-redis provider
        // does not, so we refer to our new bucket in the line below with an empty string.
        let bucket = wasi::keyvalue::store::open("").expect("failed to open empty bucket");
        let count = wasi::keyvalue::atomics::increment(&bucket, &object_name, 1)
            .expect("failed to increment count");

        // Build & send HTTP response
        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let response_body = response.body().unwrap();
        response_body
            .write()
            .unwrap()
            .blocking_write_and_flush(format!("Counter {object_name}: {count}\n").as_bytes())
            .unwrap();
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
        ResponseOutparam::set(response_out, Ok(response));
    }
}

/// Authorize an incoming request by comparing the provided secret on the HTTP header to
/// the secret stored as the API password.
fn user_is_authorized(request: &IncomingRequest) -> bool {
    let Some(provided_password) = get_header(&request, "password".to_string()) else {
        return false;
    };
    // Resource secret, not actually loaded
    let password = wasmcloud::secrets::store::get("api_password").expect("failed to get password");
    // Revealed secret
    match wasmcloud::secrets::reveal::reveal(&password) {
        wasmcloud::secrets::store::SecretValue::String(s) if s == provided_password => true,
        wasmcloud::secrets::store::SecretValue::Bytes(b)
            if String::from_utf8_lossy(&b) == provided_password =>
        {
            true
        }
        _ => false,
    }
}

fn get_header(request: &IncomingRequest, header: String) -> Option<String> {
    let raw_header = request.headers().get(&header);
    if raw_header.first().map(|val| val.is_empty()).unwrap_or(true) {
        return None;
    }
    let opt_header = raw_header.first();
    // We can unwrap here because we know that the header is set and has a value
    match std::str::from_utf8(opt_header.unwrap()) {
        Ok(s) => Some(s.to_string()),
        Err(_) => None,
    }
}

export!(KeyvalueCounterAuth);
