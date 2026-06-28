//! Minimal P3 fixture: exports a plain-value async `run` function. Paired with
//! `ephemeral-caller-p3`, which imports this interface through the dynamic
//! linker. Because the signature is all plain values, the linked call is
//! dispatched via the ephemeral-store path (`invoke_ephemeral_linked_export`).

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "export:wasmcloud:ephemeral-test/compute@0.1.0#run",
        ],
    });
}

use bindings::exports::wasmcloud::ephemeral_test::compute::Guest;

struct Component;

impl Guest for Component {
    async fn run(n: u32) -> u32 {
        n.wrapping_mul(2).wrapping_add(1)
    }
}

bindings::export!(Component with_types_in bindings);
