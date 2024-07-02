#![allow(clippy::missing_safety_doc)]

wit_bindgen::generate!("component");

use wasmcloud_component::wasi;

use exports::test_components::testing::*;

struct Actor;

impl pingpong::Guest for Actor {
    fn ping() -> String {
        wasi::config::runtime::get("pong")
            .expect("Unable to fetch value")
            .unwrap_or_else(|| "config value not set".to_string())
    }

    fn ping_secret() -> String {
        let secret = wasmcloud::secrets::store::get("ponger").expect("Unable to fetch value");
        match wasmcloud::secrets::reveal::reveal(&secret) {
            wasmcloud::secrets::store::SecretValue::String(s) => s,
            wasmcloud::secrets::store::SecretValue::Bytes(bytes) => {
                String::from_utf8_lossy(&bytes).to_string()
            }
        }
    }
}

export!(Actor);
