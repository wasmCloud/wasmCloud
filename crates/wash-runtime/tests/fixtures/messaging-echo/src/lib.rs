use crate::bindings::wasmcloud::messaging::{consumer, types::BrokerMessage};

mod bindings {
    use crate::Component;

    wit_bindgen::generate!({
        world: "echo",
        generate_all
    });

    export!(Component);
}

struct Component;

impl bindings::exports::wasmcloud::messaging::handler::Guest for Component {
    fn handle_message(msg: BrokerMessage) -> Result<(), String> {
        if let Some(reply_to) = msg.reply_to {
            let reply = BrokerMessage {
                subject: reply_to,
                body: msg.body,
                reply_to: None,
            };
            consumer::publish(&reply)?;
        }
        Ok(())
    }
}
