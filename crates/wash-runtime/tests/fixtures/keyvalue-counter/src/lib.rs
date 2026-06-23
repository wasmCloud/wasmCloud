//! Minimal keyvalue fixture.
//!
//! Imports `wasi:keyvalue` once (unnamed/default instance) and exports an HTTP
//! handler. Each request increments a counter in the default keyvalue store and
//! returns the new value as the response body, so the test can observe that the
//! unnamed import still routes to (and persists in) the standalone backend even
//! when a multiplexed `(implements ..)` keyvalue plugin is also registered.

use anyhow::{Context, Result};

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::{
        http::types::{
            Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
        },
        keyvalue::{atomics::increment, store::open},
    },
};

struct Component;

const BUCKET: &str = "counter";
const KEY: &str = "n";

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let (status, body_text) = match handle_request() {
            Ok(count) => (200, count.to_string()),
            Err(e) => (500, format!("error: {e:#}")),
        };

        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(status).unwrap();
        let body = response.body().unwrap();
        ResponseOutparam::set(response_out, Ok(response));

        let stream = body.write().unwrap();
        stream.blocking_write_and_flush(body_text.as_bytes()).unwrap();
        drop(stream);
        OutgoingBody::finish(body, None).unwrap();
    }
}

fn handle_request() -> Result<u64> {
    // Unnamed `open` -> the default keyvalue instance, served by the standalone
    // plugin even when a multiplexed plugin is registered for named imports.
    let bucket = open(BUCKET).context("open default keyvalue bucket")?;
    let count = increment(&bucket, KEY, 1).context("increment counter")?;
    Ok(count)
}

bindings::export!(Component with_types_in bindings);
