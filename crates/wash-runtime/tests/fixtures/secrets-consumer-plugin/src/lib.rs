//! Host component plugin that imports the native `wasmcloud:secrets`
//! capability and re-exports one value from it as a bespoke capability —
//! proving a host component plugin can depend on a host native builtin
//! directly. The import is resolved at plugin-load time against the host's
//! registered natives, never against another component plugin.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "secrets-consumer-plugin", generate_all });
}

use bindings::exports::acme::secretsproxy::store::Guest;
use bindings::wasmcloud::secrets::reveal::reveal;
use bindings::wasmcloud::secrets::store::{self, SecretValue};

struct Component;

impl Guest for Component {
    async fn get_secret(key: String) -> Option<String> {
        let secret = store::get(key).await.ok()?;
        match reveal(&secret).await {
            SecretValue::String(value) => Some(value),
            SecretValue::Bytes(bytes) => String::from_utf8(bytes).ok(),
        }
    }

    async fn get_db_password() -> Option<String> {
        // `db-password` is a labeled import so which secret this.
        // The host already checked it resolves before this component was instantiated.
        let secret = bindings::db_password::get().await;
        match reveal(&secret).await {
            SecretValue::String(value) => Some(value),
            SecretValue::Bytes(bytes) => String::from_utf8(bytes).ok(),
        }
    }
}

mod export {
    #![allow(unsafe_code)]
    use super::{Component, bindings};
    bindings::export!(Component with_types_in bindings);
}
