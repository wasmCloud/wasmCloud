//! P3 fixture: receives a `token` resource handle as a parameter and calls a
//! method on it (dispatched back to `res-producer-p3` through the linker).
//! The inbound handle is lowered across the dynamic linker via
//! `lower_with_type` — if identity were not preserved, `greet()` below would
//! fail or read the wrong resource.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "export:wasmcloud:resource-test/sink@0.1.0#accept",
            "export:wasmcloud:resource-test/sink@0.1.0#bounce",
        ],
    });
}

use bindings::exports::wasmcloud::resource_test::sink::Guest;
use bindings::wasmcloud::resource_test::factory::Token;

struct Component;

impl Guest for Component {
    async fn bounce(t: Token) -> Token {
        t
    }

    async fn accept(t: Token) -> String {
        let greeting = format!("sink:{}", t.greet());
        drop(t);
        let environment = bindings::wasi::cli::environment::get_environment();
        assert!(environment
            .iter()
            .any(|(k, v)| k == "RESOURCE_DROP_CONTEXT" && v == "res-sink"));
        greeting
    }
}

bindings::export!(Component with_types_in bindings);
