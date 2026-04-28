use crate::bindings::{wasi::logging::logging, wasmcloud::messaging::types::BrokerMessage};

mod bindings {
    use crate::Component;

    wit_bindgen::generate!({
        world: "hello",
        generate_all
    });

    export!(Component);
}

struct Component;

impl bindings::exports::wasmcloud::messaging::handler::Guest for Component {
    fn handle_message(_msg: BrokerMessage) -> Result<(), String> {
        logging::log(logging::Level::Info, "messaging", "hello, world!");
        Ok(())
    }
}
