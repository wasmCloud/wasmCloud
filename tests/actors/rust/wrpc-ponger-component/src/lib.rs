wit_bindgen::generate!({
    world: "test",
    exports: {
        "wrpc:testing/pingpong": Component,
    },
});

use exports::wrpc::testing::pingpong::Guest;

struct Component;

impl Guest for Component {
    fn ping() -> String {
        "pong".to_string()
    }
}
