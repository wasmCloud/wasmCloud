wit_bindgen::generate!({
    path: "../wit",
    world: "task",
    with: {
        "wasmcloud:messaging/types@0.2.0": generate,
        "wasmcloud:messaging/consumer@0.2.0": generate,
    },
});

use crate::wasmcloud::messaging::types::BrokerMessage;
use wasmcloud::messaging::consumer;
#[allow(unused)]
use wstd::prelude::*;

struct Component;
export!(Component);

impl exports::wasmcloud::messaging::handler::Guest for Component {
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
