mod bindings {
    wit_bindgen::generate!({ generate_all });
}

#[path = "../../tls-echo-client-p3/src/echo.rs"]
mod echo;

struct Component;

impl bindings::exports::wasi::cli::run::Guest for Component {
    async fn run() -> Result<(), ()> {
        let addr = bindings::wasi::config::store::get("echo-addr")
            .map_err(|_| ())?
            .ok_or(())?;
        echo::round_trip(&addr).await
    }
}

bindings::export!(Component with_types_in bindings);
