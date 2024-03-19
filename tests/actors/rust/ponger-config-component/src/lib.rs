#![allow(clippy::missing_safety_doc)]

wit_bindgen::generate!("actor");

use exports::test_actors::testing::*;

struct Actor;

impl pingpong::Guest for Actor {
    fn ping() -> String {
        wasmcloud::bus::guest_config::get("pong")
            .expect("Unable to fetch value")
            .map(|val| String::from_utf8(val).expect("config value should be valid string"))
            .unwrap_or_else(|| "config value not set".to_string())
    }
}

export!(Actor);
