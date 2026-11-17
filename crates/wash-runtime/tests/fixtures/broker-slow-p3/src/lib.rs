mod bindings {
    wit_bindgen::generate!({ world: "broker-slow-p3", generate_all });
}

use bindings::exports::wasmcloud::broker_test::slow::Guest;

struct Component;

impl Guest for Component {
    async fn wait(ms: u32, _held: wit_bindgen::StreamReader<u8>) {
        bindings::wasi::clocks::monotonic_clock::wait_for(u64::from(ms) * 1_000_000).await;
    }
}

bindings::export!(Component with_types_in bindings);
