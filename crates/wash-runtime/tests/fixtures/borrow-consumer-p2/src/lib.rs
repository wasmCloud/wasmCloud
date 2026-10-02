//! P2 fixture, the last hop. Receives `borrow<token>` and calls `greet` on it,
//! which the linker dispatches back to the producer.

mod bindings {
    wit_bindgen::generate!({
        world: "consumer-component",
        generate_all,
    });
}

use bindings::exports::wasmcloud::borrow_test::consumer::Guest;
use bindings::wasmcloud::borrow_test::factory::{Lend, Token};

struct Component;

impl Guest for Component {
    fn accept(t: &Token) -> String {
        format!("consumer:{}", t.greet())
    }

    fn accept_nested(l: Lend<'_>, pair: (String, &Token), maybe: Option<&Token>) -> String {
        let mut greets = vec![l.token.greet(), pair.1.greet()];
        greets.extend(maybe.map(|t| t.greet()));
        format!("consumer:{}:{}:{}", l.label, pair.0, greets.join(","))
    }

    fn adopt((label, t): (String, Token)) -> String {
        format!("consumer:{label}:{}", t.greet())
    }
}

bindings::export!(Component with_types_in bindings);
