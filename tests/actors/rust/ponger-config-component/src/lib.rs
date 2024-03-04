wit_bindgen::generate!({
    world: "actor",
    exports: {
        "test-actors:testing/pingpong": Component,
    },
});

use exports::test_actors::testing::*;

struct Component;

impl pingpong::Guest for Component {
    fn ping() -> String {
        wasmcloud::bus::guest_config::get("pong")
            .expect("Unable to fetch value")
            .map(|val| String::from_utf8(val).expect("config value should be valid string"))
            .unwrap_or_else(|| "config value not set".to_string())
    }
}
