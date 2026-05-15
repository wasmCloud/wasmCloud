mod bindings {
    wit_bindgen::generate!({
        path: "../wit",
        world: "task",
        generate_all,
    });
}

use bindings::wasmcloud::messaging::consumer;
use bindings::wasmcloud::messaging::types::BrokerMessage;
#[allow(unused)]
use wstd::prelude::*;

struct Component;

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

        let reply = BrokerMessage {
            subject,
            body: to_leet_speak(&payload).into(),
            reply_to: None,
        };

        consumer::publish(&reply)
    }
}

fn to_leet_speak(input: &str) -> String {
    input
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            'a' => '4',
            'e' => '3',
            'i' => '1',
            'o' => '0',
            's' => '5',
            't' => '7',
            'l' => '1',
            _ => c,
        })
        .collect()
}
