wit_bindgen::generate!({ generate_all });

use exports::wasmcloud::messaging::incoming_handler::Guest;
use wasmcloud::messaging::request_reply::reply;
use wasmcloud::messaging::types::{Error, Message};

struct Echo;

impl Guest for Echo {
    fn handle(msg: Message) -> Result<(), Error> {
        reply(&msg, Message::new(&msg.data()))
    }
}

export!(Echo);
