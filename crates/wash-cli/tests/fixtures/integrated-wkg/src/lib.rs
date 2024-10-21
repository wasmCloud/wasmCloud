wit_bindgen::generate!({ generate_all });

use exports::test_components::testing::*;

struct Component;

impl pingpong::Guest for Component {
    fn ping() -> String {
        "helloworld".to_string()
    }

    fn ping_secret() -> String {
        "helloworld".to_string()
    }
}

export!(Component);
