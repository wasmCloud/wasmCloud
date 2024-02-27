wit_bindgen::generate!({
    world: "test",
    exports: {
        "wrpc:testing/invoke": Component,
    },
});

use exports::wrpc::testing::invoke::Guest;

struct Component;

impl Guest for Component {
    fn call() -> String {
        let pong = crate::wrpc::testing::pingpong::ping();
        format!("Ping {pong}")
    }
}
