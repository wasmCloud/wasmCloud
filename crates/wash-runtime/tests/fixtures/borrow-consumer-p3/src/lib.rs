//! P3 fixture, the last hop. Receives `borrow<token>` and calls `greet` on it,
//! which the linker dispatches back to the producer.

mod bindings {
    wit_bindgen::generate!({
        world: "consumer-component",
        generate_all,
        async: [
            "export:wasmcloud:borrow-test-p3/consumer@0.1.0#accept",
        ],
    });
}

use bindings::exports::wasmcloud::borrow_test_p3::consumer::Guest;
use bindings::wasmcloud::borrow_test_p3::factory::Token;

struct Component;

impl Guest for Component {
    async fn accept(t: &Token) -> String {
        format!("consumer:{}", t.greet())
    }
}

bindings::export!(Component with_types_in bindings);
