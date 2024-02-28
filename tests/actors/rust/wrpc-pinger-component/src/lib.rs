wit_bindgen::generate!({
    world: "actor",
    exports: {
        "wasmcloud:testing/invoke": Actor,
    },
});

use crate::wasmcloud::testing::*;
use exports::wasmcloud::testing::invoke::Guest;

struct Actor;

impl Guest for Actor {
    fn call() -> String {
        // No args, return string
        let pong = pingpong::ping();
        // Number arg, return number
        let meaning_of_universe = busybox::increment_number(41);
        // Multiple args, return vector of strings
        let other: Vec<String> = busybox::string_split("hi,there,friend", ',');
        // Variant / Enum argument, return bool
        let is_same = busybox::string_assert(busybox::Easyasonetwothree::A, "a");
        let doggo = busybox::Dog {
            name: "Archie".to_string(),
            age: 3,
        };
        // Record / struct argument
        let is_good_boy = busybox::is_good_boy(&doggo);
        format!("Ping {pong}, meaning of universe is: {meaning_of_universe}, split: {other:?}, is_same: {is_same}, archie good boy: {is_good_boy}")
    }
}
