wit_bindgen::generate!({
    world: "actor",
    exports: {
        "wasmcloud:testing/pingpong": Component,
        "wasmcloud:testing/busybox": Component,
    },
});

use exports::wasmcloud::testing::*;

struct Component;

impl pingpong::Guest for Component {
    fn ping() -> String {
        "pong".to_string()
    }
}

impl busybox::Guest for Component {
    #[doc = " increments a number"]
    fn increment_number(num: u32) -> u32 {
        num.saturating_add(1)
    }

    #[doc = " split a string based on a char delimiter"]
    fn string_split(str: String, del: char) -> Vec<String> {
        str.split(del)
            .map(|s| s.to_string())
            .collect::<Vec<String>>()
    }

    #[doc = " Assert that a String matches the variant"]
    fn string_assert(letter: busybox::Easyasonetwothree, test: String) -> bool {
        match letter {
            busybox::Easyasonetwothree::A => test == "a",
            busybox::Easyasonetwothree::B => test == "b",
            busybox::Easyasonetwothree::C => test == "c",
        }
    }

    fn is_good_boy(dog: busybox::Dog) -> bool {
        use crate::wasi::logging::logging::*;
        log(
            Level::Info,
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
