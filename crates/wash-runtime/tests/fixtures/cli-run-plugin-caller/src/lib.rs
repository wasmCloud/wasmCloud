//! Workload component that turns a request into a call on the plugin's
//! `acme:clirun/control`, so the plugin's `wasi:cli/run` dispatching is
//! drivable from an ordinary HTTP test:
//!
//! - `GET /run-workload?id=W` -> `control.run-workload(W)` -> `ok`/`err`/`unroutable:W`
//! - `GET /run-inherited`     -> `control.run-inherited()` -> `ok`/`err`
//! - `GET /dispatched`        -> `control.dispatched()`    -> `id=count|…`
//! - `GET /probe?id=W`        -> `control.probe-workload(W)` -> `ok:…`/`err`

mod bindings;

use bindings::acme::clirun::control;
use bindings::exports::acme::clirun::probe::Guest as ProbeGuest;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

impl ProbeGuest for Component {
    /// Called by the plugin across the store boundary. `PROBE_MODE=trap` traps
    /// instead of answering, so a test can prove the host reports that through
    /// this interface's payload-less error arm rather than faulting the store
    /// the plugin shares with every workload it serves.
    async fn check() -> Result<String, ()> {
        let mode = std::env::var("PROBE_MODE").unwrap_or_default();
        assert!(mode != "trap", "deliberate trap for the result<T> test");
        Ok(format!("checked:{mode}"))
    }
}

impl HttpGuest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request
            .get_path_with_query()
            .unwrap_or_else(|| "/".to_string());
        let (route, query) = match path.split_once('?') {
            Some((r, q)) => (r, q),
            None => (path.as_str(), ""),
        };

        let body = if route.starts_with("/run-workload") {
            let id = query_get(query, "id").unwrap_or_default();
            control::run_workload(id).await
        } else if route.starts_with("/run-inherited") {
            control::run_inherited().await
        } else if route.starts_with("/dispatched") {
            control::dispatched().await.join("|")
        } else if route.starts_with("/probe") {
            let id = query_get(query, "id").unwrap_or_default();
            control::probe_workload(id).await
        } else {
            return Ok(make_response(404, Vec::new()));
        };

        Ok(make_response(200, body.into_bytes()))
    }
}

/// First value of `key` in a `a=b&c=d` query string.
fn query_get(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn make_response(status: u16, body: Vec<u8>) -> Response {
    let headers = Fields::new();
    let _ = headers.set(&"content-type".to_string(), &[b"text/plain".to_vec()]);
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        tx.write_all(body).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    let _ = response.set_status_code(status);
    response
}

bindings::export!(Component with_types_in bindings);
