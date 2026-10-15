//! P2 fixture, the HTTP entrypoint. Hands host resources to the callee, which
//! runs in a store of its own: the request body's stream and the response
//! body's, both lent and given away, then header fields and the request itself.

mod bindings {
    wit_bindgen::generate!({
        world: "caller",
        generate_all,
    });
}

use bindings::exports::wasi::http::incoming_handler::Guest;
use bindings::wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};
use bindings::wasi::io::streams::OutputStream;
use bindings::wasmcloud::lend_test::relay;

struct Component;

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

impl Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let path = request.path_with_query().unwrap_or_default();
        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let out_body = response.body().unwrap();
        ResponseOutparam::set(response_out, Ok(response));

        let out = out_body.write().unwrap();
        match path.as_str() {
            "/streams" => streams(request, out),
            "/request" => whole_request(request, out),
            _ => out.blocking_write_and_flush(b"unknown route").unwrap(),
        }
        OutgoingBody::finish(out_body, None).unwrap();
    }
}

/// Lend the request body's stream, read from it again, give it away and get it
/// back, then give it away for good. The response body's stream is lent,
/// written to here, and given away.
fn streams(request: IncomingRequest, out: OutputStream) {
    let body = request.consume().unwrap();
    let data = body.stream().unwrap();

    let peeked = relay::peek(&data, 5);
    let after_loan = text(data.blocking_read(1).unwrap());
    let data = relay::pass(data);
    let after_return = text(data.blocking_read(3).unwrap());
    let drained = relay::drain(data);

    relay::write(&out, &format!("{peeked}|{after_loan}|{after_return}|{drained}|"));
    out.blocking_write_and_flush(b"caller|").unwrap();
    relay::finish(out, "callee");
}

/// Lend the request's headers, once and then twice over in one call, and keep
/// using them. Then give the request away.
fn whole_request(request: IncomingRequest, out: OutputStream) {
    let headers = request.headers();
    let lent = relay::header(&headers, "x-probe");
    let twice = relay::both(&headers, &headers, "x-probe");
    let kept = headers.get("x-probe").into_iter().map(text);
    let kept = kept.collect::<Vec<_>>().join(",");
    drop(headers);

    let described = relay::describe(request);
    out.blocking_write_and_flush(format!("{lent}|{twice}|{kept}|{described}").as_bytes())
        .unwrap();
}

bindings::export!(Component with_types_in bindings);
