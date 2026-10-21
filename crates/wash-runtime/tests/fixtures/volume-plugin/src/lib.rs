//! Host component plugin that touches the filesystem with plain `std::fs`.
//! Its store has no filesystem of its own, so every path it can reach is one
//! its `volumes` preopened — which is what the volume tests assert.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "volume-plugin", generate_all });
}

use bindings::exports::acme::volume::files::Guest;

struct Component;

impl Guest for Component {
    async fn read(path: String) -> String {
        match std::fs::read_to_string(&path) {
            Ok(contents) => format!("ok:{contents}"),
            Err(e) => format!("error: {e}"),
        }
    }

    async fn write(path: String, contents: String) -> String {
        match std::fs::write(&path, contents) {
            Ok(()) => "ok".to_string(),
            Err(e) => format!("error: {e}"),
        }
    }
}

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
