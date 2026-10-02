//! P3 fixture, the middle hop. Receives `borrow<token>` and lends the same
//! handle on to the consumer. Both the inbound and the outbound handle are host
//! `ResourceAny`s, so the linker lowers the outbound one by identity.

mod bindings {
    wit_bindgen::generate!({
        world: "middleware-component",
        generate_all,
        async: [
            "import:wasmcloud:borrow-test-p3/consumer@0.1.0#accept",
            "export:wasmcloud:borrow-test-p3/middleware@0.1.0#accept",
            "import:wasmcloud:borrow-test-p3/consumer@0.1.0#accept-nested",
            "export:wasmcloud:borrow-test-p3/middleware@0.1.0#accept-nested",
            "import:wasmcloud:borrow-test-p3/consumer@0.1.0#adopt",
            "export:wasmcloud:borrow-test-p3/middleware@0.1.0#adopt",
        ],
    });
}

use bindings::exports::wasmcloud::borrow_test_p3::middleware::Guest;
use bindings::wasmcloud::borrow_test_p3::factory::{Lend, Token};
use bindings::wasmcloud::borrow_test_p3::consumer;

struct Component;

impl Guest for Component {
    async fn accept(t: &Token) -> String {
        format!("middleware:{}", consumer::accept(t).await)
    }

    async fn accept_nested(l: Lend<'_>, pair: (String, &Token), maybe: Option<&Token>) -> String {
        format!(
            "middleware:{}",
            consumer::accept_nested(l, pair, maybe).await
        )
    }

    async fn adopt(t: (String, Token)) -> String {
        format!("middleware:{}", consumer::adopt(t).await)
    }
}

bindings::export!(Component with_types_in bindings);
