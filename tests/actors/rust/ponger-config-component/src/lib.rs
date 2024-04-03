#![allow(clippy::missing_safety_doc)]

wit_bindgen::generate!("actor");

use exports::test_actors::testing::*;

struct Actor;

impl pingpong::Guest for Actor {
    fn ping() -> String {
        wasi::config::runtime::get("pong")
            .expect("Unable to fetch value")
            .unwrap_or_else(|| "config value not set".to_string())
    }
}

export!(Actor);
