//! P3 fixture: produces a guest `token` resource. Paired with `res-sink-p3`
//! and `res-caller-p3` to exercise passing a resource handle across the
//! dynamic linker (`engine::value::lower_with_type` identity passthrough).

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "export:wasmcloud:resource-test/factory@0.1.0#make-token",
        ],
    });
}

use bindings::exports::wasmcloud::resource_test::factory::{Guest, GuestToken, Token};

struct Component;

struct TokenState {
    name: String,
}

impl GuestToken for TokenState {
    fn greet(&self) -> String {
        format!("hello {}", self.name)
    }
}

impl Guest for Component {
    type Token = TokenState;

    async fn make_token(name: String) -> Token {
        Token::new(TokenState { name })
    }
}

bindings::export!(Component with_types_in bindings);
