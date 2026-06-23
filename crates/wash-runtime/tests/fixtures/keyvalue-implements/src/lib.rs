//! Real-guest fixture for `(implements ..)` named keyvalue routing.
//!
//! Imports `wasi:keyvalue/store` twice under the labels `team-a` and `team-b`
//! (the WIT `implements` clause). On each HTTP request it writes a distinct
//! value to the same key through each named import and reads both back; the host
//! routes the two labels to separate backends, so the values must stay isolated.
//! Responds `isolated` on success, `leak: …` otherwise.

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

const KEY: &str = "k";

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let body = match run() {
            Ok(s) => s,
            Err(e) => format!("error: {e}"),
        };

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

fn run() -> Result<String, String> {
    // `team-a` and `team-b` are two independent `(implements ..)` imports of
    // wasi:keyvalue/store, routed by the host to separate backends.
    let a = bindings::team_a::open("bucket").map_err(|e| format!("open team-a: {e:?}"))?;
    let b = bindings::team_b::open("bucket").map_err(|e| format!("open team-b: {e:?}"))?;

    a.set(KEY, b"from-a").map_err(|e| format!("set team-a: {e:?}"))?;
    b.set(KEY, b"from-b").map_err(|e| format!("set team-b: {e:?}"))?;

    let va = a.get(KEY).map_err(|e| format!("get team-a: {e:?}"))?;
    let vb = b.get(KEY).map_err(|e| format!("get team-b: {e:?}"))?;

    if va.as_deref() == Some(b"from-a".as_slice()) && vb.as_deref() == Some(b"from-b".as_slice()) {
        Ok("isolated".to_string())
    } else {
        Ok(format!("leak: team-a={va:?} team-b={vb:?}"))
    }
}

bindings::export!(Component with_types_in bindings);
