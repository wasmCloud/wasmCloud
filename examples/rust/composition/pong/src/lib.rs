wit_bindgen::generate!({ generate_all });

use std::collections::HashMap;

use exports::example::pong::pingpong::Guest;
use wasi::cli::environment::get_environment;

struct Pong;

impl Guest for Pong {
    fn ping() -> String {
        let mut env: HashMap<String, String> = get_environment().into_iter().collect();
        env.remove("PONG").unwrap_or_else(|| "pong".to_string())
    }
}

export!(Pong);
