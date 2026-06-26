//! Real-guest fixture for `(implements ..)` named postgres routing.
//!
//! Imports `wasmcloud:postgres/query` twice under the labels `team-a` and
//! `team-b`. The host routes each label to a connection with that team's
//! database credentials.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::incoming_handler::Guest;
use bindings::wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

struct Component;

const SELECT: &str = "SELECT val FROM team_a_data WHERE id = 1";

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let body = run();

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

fn run() -> String {
    // Both labels point at the same postgres instance but with different
    // credentials: team-a owns `team_a_data`, team-b has no grant on it.
    let a_ok = bindings::team_a::query(SELECT, &[])
        .map(|rows| !rows.is_empty())
        .unwrap_or(false);
    let b_denied = bindings::team_b::query(SELECT, &[]).is_err();

    if a_ok && b_denied {
        "isolated".to_string()
    } else {
        format!("leak: team-a-ok={a_ok} team-b-denied={b_denied}")
    }
}

bindings::export!(Component with_types_in bindings);
