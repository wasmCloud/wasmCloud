//! P2 fixture, the HTTP entrypoint. Creates a `token` in the producer, lends
//! it to the middleware (which lends it on to the consumer), then keeps using
//! it. The body reports the middleware's reply, the caller's own `greet`, and
//! the producer's greet count.

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
use bindings::wasmcloud::borrow_test::factory::Token;
use bindings::wasmcloud::borrow_test::middleware;

struct Component;

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        // `token` is a resource living in the producer; the caller holds the
        // owned handle.
        let token = Token::new("world");
        // Lend it: caller -> middleware -> consumer, where `greet` runs on it.
        let via_middleware = middleware::accept(&token);
        // The caller still owns the token and can keep using it.
        let direct = token.greet();
        let body = format!("{via_middleware}|{direct}|greets={}", token.greets());

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
