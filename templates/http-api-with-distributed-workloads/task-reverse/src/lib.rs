mod bindings {
    wit_bindgen::generate!({
        path: "../wit",
        world: "task",
        generate_all,
    });
}

use bindings::wasi::config::store;
use bindings::wasmcloud::messaging::consumer;
use bindings::wasmcloud::messaging::types::BrokerMessage;
#[allow(unused)]
use wstd::prelude::*;

struct Component;

/// Per-worker behavior, read from `wasi:config/store`. Values come from the
/// workload-level `config:` block, with this component's `dev.components`
/// entry overriding on a per-key basis (see `.wash/config.yaml`).
struct Settings {
    /// `chars` reverses the characters; `words` reverses the word order.
    by_words: bool,
    /// Prepended to every reply. Empty by default.
    prefix: String,
}

impl Settings {
    fn load() -> Self {
        // `get` returns Ok(None) when unset and Err only on a store fault;
        // fall back to the character-reversal, unprefixed defaults either way.
        let get = |key: &str| store::get(key).ok().flatten();
        Settings {
            by_words: get("reverse.mode").as_deref() == Some("words"),
            prefix: get("reverse.prefix").unwrap_or_default(),
        }
    }
}

#[allow(unsafe_code)] // bindings::export! emits unsafe FFI shims
mod export {
    use super::{Component, bindings};
    bindings::export!(Component with_types_in bindings);
}

impl bindings::exports::wasmcloud::messaging::handler::Guest for Component {
    fn handle_message(msg: BrokerMessage) -> Result<(), String> {
        let Some(subject) = msg.reply_to else {
            return Err("missing reply_to".to_string());
        };

        let payload = String::from_utf8(msg.body.to_vec())
            .map_err(|e| format!("Failed to decode message body: {}", e))?;

        let settings = Settings::load();
        let reply = BrokerMessage {
            subject,
            body: format!("{}{}", settings.prefix, reverse(&payload, &settings)).into(),
            reply_to: None,
        };

        consumer::publish(&reply)
    }
}

fn reverse(input: &str, settings: &Settings) -> String {
    if settings.by_words {
        input.split_whitespace().rev().collect::<Vec<_>>().join(" ")
    } else {
        input.chars().rev().collect()
    }
}
