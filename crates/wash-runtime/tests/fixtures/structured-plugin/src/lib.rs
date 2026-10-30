//! Forward structured async values through real guest code.

#[allow(unsafe_code)]
mod bindings {
    wit_bindgen::generate!({ world: "plugin", generate_all });
}

use bindings::test::payload::types::Entry;
use wit_bindgen::{FutureReader, StreamReader};

struct Component;
impl bindings::exports::test::payload::provider::Guest for Component {
    async fn relay(
        input: StreamReader<Entry>,
    ) -> (StreamReader<Entry>, FutureReader<Result<Entry, String>>) {
        bindings::test::payload::native::relay(input).await
    }
}

#[allow(unsafe_code)]
mod export {
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
