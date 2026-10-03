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

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

static CREATED: AtomicU32 = AtomicU32::new(0);
static DROPPED: AtomicU32 = AtomicU32::new(0);
static WRONG_CONTEXT: AtomicU32 = AtomicU32::new(0);

struct Component;

struct TokenState {
    name: String,
}

impl Drop for TokenState {
    fn drop(&mut self) {
        DROPPED.fetch_add(1, Relaxed);
        // Call the host directly: std::env / the P1 adapter may cache values.
        let environment = bindings::wasi::cli::environment::get_environment();
        if !environment
            .iter()
            .any(|(k, v)| k == "RESOURCE_DROP_CONTEXT" && v == "res-producer")
        {
            WRONG_CONTEXT.fetch_add(1, Relaxed);
        }
    }
}

impl GuestToken for TokenState {
    fn greet(&self) -> String {
        format!("hello {}", self.name)
    }
}

impl Guest for Component {
    type Token = TokenState;

    async fn make_token(name: String) -> Token {
        CREATED.fetch_add(1, Relaxed);
        Token::new(TokenState { name })
    }

    fn stats() -> (u32, u32, u32) {
        (
            CREATED.load(Relaxed),
            DROPPED.load(Relaxed),
            WRONG_CONTEXT.load(Relaxed),
        )
    }
}

bindings::export!(Component with_types_in bindings);
