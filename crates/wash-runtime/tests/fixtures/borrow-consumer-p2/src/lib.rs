//! P2 fixture, the last hop. Receives `borrow<token>` and calls `greet` on it,
//! which the linker dispatches back to the producer.

mod bindings {
    wit_bindgen::generate!({
        world: "consumer-component",
        generate_all,
    });
}

use bindings::exports::wasmcloud::borrow_test::consumer::Guest;
use bindings::wasmcloud::borrow_test::factory::Token;

struct Component;

impl Guest for Component {
    fn accept(t: &Token) -> String {
        format!("consumer:{}", t.greet())
    }
}

bindings::export!(Component with_types_in bindings);
