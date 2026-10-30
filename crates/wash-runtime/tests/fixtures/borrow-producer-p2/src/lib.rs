//! P2 fixture, owner of the `token` resource. The caller creates one here and
//! lends it down the chain; every `greet` on it lands back in this component.

use std::cell::Cell;

mod bindings {
    wit_bindgen::generate!({
        world: "producer",
        generate_all,
    });
}

use bindings::exports::wasmcloud::borrow_test::factory::{Guest, GuestToken};

struct Component;

impl Guest for Component {
    type Token = Token;
}

pub struct Token {
    name: String,
    greets: Cell<u32>,
}

impl GuestToken for Token {
    fn new(name: String) -> Self {
        Self {
            name,
            greets: Cell::new(0),
        }
    }

    fn greet(&self) -> String {
        self.greets.set(self.greets.get() + 1);
        format!("hello {}", self.name)
    }

    fn greets(&self) -> u32 {
        self.greets.get()
    }
}

bindings::export!(Component with_types_in bindings);
