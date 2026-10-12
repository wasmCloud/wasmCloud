//! Middle hop for the nested active_ctx test: `relay.hold` starts a linked
//! call into `gate` (C), parks it, reports its OWN context while C is in
//! flight, waits for the caller's release, then lets C finish.
mod bindings {
    wit_bindgen::generate!({ world: "ctx-race-middle", generate_all });
}

use std::pin::pin;

use bindings::exports::wasmcloud::ctx_race::relay::Guest;
use bindings::wasi::cli::environment::get_environment;
use bindings::wasi::http::client::send;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Scheme};
use bindings::wasmcloud::ctx_race::gate;
use wit_bindgen::FutureReader;

struct Component;

fn who() -> String {
    get_environment()
        .into_iter()
        .find(|(k, _)| k == "WHO")
        .map(|(_, v)| v)
        .unwrap_or_else(|| "?".into())
}

async fn egress() -> u16 {
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    let (request, _sent) = Request::new(Fields::new(), None, trailers_rx, None);
    wit_bindgen::spawn_local(async move {
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let _ = request.set_scheme(Some(&Scheme::Http));
    let _ = request.set_authority(Some("example.com"));
    let _ = request.set_path_with_query(Some("/"));
    match send(request).await {
        Ok(_) => 200,
        Err(ErrorCode::HttpRequestDenied) => 403,
        Err(_) => 502,
    }
}

async fn snapshot() -> String {
    format!("{{\"who\":\"{}\",\"egress\":{}}}", who(), egress().await)
}

impl Guest for Component {
    async fn hold(release: FutureReader<u32>) -> String {
        let before = snapshot().await;
        let (tx, rx) = bindings::wit_future::new::<u32>(|| 0);
        let mut c = pin!(gate::hold(rx));
        assert!(futures::poll!(c.as_mut()).is_pending());
        let during = snapshot().await;
        let _ = release.await;
        let _ = tx.write(1).await;
        let c = c.await;
        let after = snapshot().await;
        format!("{{\"before\":{before},\"during\":{during},\"c\":\"{c}\",\"after\":{after}}}")
    }
}

bindings::export!(Component with_types_in bindings);
