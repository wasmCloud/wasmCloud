wit_bindgen::generate!({
    with: {
        "wasmcloud:messaging/types@0.2.0": wasmcloud_component::wasmcloud::messaging0_2_0::types,
        "wasmcloud:messaging/types@0.3.0": wasmcloud_component::wasmcloud::messaging0_3_0::types,
    },
    generate_all,
});

use exports::test_components::testing;

use wasmcloud_component::wasi::logging::logging;
use wasmcloud_component::wasmcloud::{messaging0_2_0, messaging0_3_0};

struct Actor;

impl testing::pingpong::Guest for Actor {
    fn ping() -> String {
        "pong".to_string()
    }

    fn ping_secret() -> String {
        "pong_secret".to_string()
    }
}

impl testing::busybox::Guest for Actor {
    fn increment_number(num: u32) -> u32 {
        num.saturating_add(1)
    }

    fn string_split(str: String, del: char) -> Vec<String> {
        str.split(del)
            .map(|s| s.to_string())
            .collect::<Vec<String>>()
    }

    fn string_assert(letter: testing::busybox::Easyasonetwothree, test: String) -> bool {
        match letter {
            testing::busybox::Easyasonetwothree::A => test == "a",
            testing::busybox::Easyasonetwothree::B => test == "b",
            testing::busybox::Easyasonetwothree::C => test == "c",
        }
    }

    fn is_good_boy(dog: testing::busybox::Dog) -> bool {
        logging::log(
            logging::Level::Info,
            "",
            &format!(
                "I PET A DOG NAMED {} WHO WAS {} YEARS OLD",
                dog.name, dog.age
            ),
        );
        // Dog is always a good boy
        true
    }
}

impl exports::wasmcloud::messaging0_2_0::handler::Guest for Actor {
    fn handle_message(
        messaging0_2_0::types::BrokerMessage {
            subject,
            body,
            reply_to,
        }: messaging0_2_0::types::BrokerMessage,
    ) -> Result<(), String> {
        let reply_to = reply_to.ok_or("`reply_to` subject missing".to_string())?;
        messaging0_2_0::consumer::publish(&messaging0_2_0::types::BrokerMessage {
            subject: reply_to,
            body,
            reply_to: Some(subject),
        })
    }
}

impl exports::wasmcloud::messaging0_3_0::incoming_handler::Guest for Actor {
    fn handle(msg: messaging0_3_0::types::Message) -> Result<(), messaging0_3_0::types::Error> {
        messaging0_3_0::request_reply::reply(&msg, messaging0_3_0::types::Message::new(&msg.data()))
    }
}

export!(Actor);
