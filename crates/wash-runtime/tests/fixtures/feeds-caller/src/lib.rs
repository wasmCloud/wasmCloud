//! Imports one guest-exported interface twice under `(implements ..)` labels.
//!
//! `feed-a` and `feed-b` name two sibling components in the same workload,
//! both exporting `example:feeds/reader@0.1.0`. On each request this
//! component calls `source` through both labels and answers with what came
//! back, so the test can see whether each label reached its own sibling.

mod bindings {
    wit_bindgen::generate!({
        world: "component",
        generate_all,
    });
}

use bindings::exports::wasi::http::incoming_handler::Guest;
use bindings::wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

struct Component;

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let body = format!("{}|{}", bindings::feed_a::source(), bindings::feed_b::source());

        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let out_body = response.body().unwrap();
        ResponseOutparam::set(response_out, Ok(response));

        let stream = out_body.write().unwrap();
        stream.blocking_write_and_flush(body.as_bytes()).unwrap();
        drop(stream);
        OutgoingBody::finish(out_body, None).unwrap();
    }
}

bindings::export!(Component with_types_in bindings);
