//! Linked callee for the `active_ctx` race tests. `hold` parks on a future the
//! caller resolves, so the caller decides exactly when each call finishes.

mod bindings {
    wit_bindgen::generate!({ world: "ctx-race-callee", generate_all });
}

use bindings::exports::wasmcloud::ctx_race::gate::Guest;
use bindings::wasi::cli::environment::get_environment;
use wit_bindgen::FutureReader;

struct Component;

fn who() -> String {
    get_environment()
        .into_iter()
        .find(|(k, _)| k == "WHO")
        .map(|(_, v)| v)
        .unwrap_or_else(|| "?".into())
}

impl Guest for Component {
    async fn hold(release: FutureReader<u32>) -> String {
        let before = who();
        let _ = release.await;
        let after = who();
        format!("{before}/{after}")
    }
}

bindings::export!(Component with_types_in bindings);
