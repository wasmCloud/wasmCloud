//! P3 fixture that takes ownership of a `token` and drops it later, from a
//! store of its own. That lets a test land the drop on the owner while the
//! owner is busy with a call from someone else.

mod bindings {
    wit_bindgen::generate!({
        world: "adopter-component",
        generate_all,
        async: [
            "export:wasmcloud:companion-test/adopter@0.1.0#adopt-after",
            "import:wasi:clocks/monotonic-clock@0.3.0#wait-for",
        ],
    });
}

use bindings::exports::wasmcloud::companion_test::adopter::Guest;
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasmcloud::companion_test::tokens::Token;

struct Component;

impl Guest for Component {
    fn ready() -> bool {
        true
    }

    async fn adopt_after(t: Token, ms: u32) {
        monotonic_clock::wait_for(u64::from(ms) * 1_000_000).await;
        drop(t);
    }
}

bindings::export!(Component with_types_in bindings);
