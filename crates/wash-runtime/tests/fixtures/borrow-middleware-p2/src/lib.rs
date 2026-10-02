//! P2 fixture, the middle hop. Receives `borrow<token>` and lends the same
//! handle on to the consumer. Both the inbound and the outbound handle are host
//! `ResourceAny`s, so the linker lowers the outbound one by identity.

mod bindings {
    wit_bindgen::generate!({
        world: "middleware-component",
        generate_all,
    });
}

use bindings::exports::wasmcloud::borrow_test::middleware::Guest;
use bindings::wasmcloud::borrow_test::factory::Token;
use bindings::wasmcloud::borrow_test::consumer;

struct Component;

impl Guest for Component {
    fn accept(t: &Token) -> String {
        format!("middleware:{}", consumer::accept(t))
    }
}

bindings::export!(Component with_types_in bindings);
