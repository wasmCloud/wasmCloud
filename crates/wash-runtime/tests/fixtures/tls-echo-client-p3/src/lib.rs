mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::cli::run::Guest;

mod echo;

struct Component;

impl Guest for Component {
    async fn run() -> Result<(), ()> {
        let addr = std::env::var("ECHO_ADDR").expect("ECHO_ADDR must be set");
        echo::round_trip(&addr).await
    }
}

bindings::export!(Component with_types_in bindings);
