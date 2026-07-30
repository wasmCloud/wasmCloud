//! Workload component driving the imported `wasmcloud:secrets` capability over
//! HTTP. The host resolves that import to the secrets host component plugin
//! running in its own store, so each request is a cross-store call the test can
//! drive by hitting an endpoint:
//!
//! - `GET /get?key=K` -> `store.get(K)` + `reveal` -> 200 body=value, or 404

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "secrets-caller", generate_all });
}

use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::secrets::reveal::reveal;
use bindings::wasmcloud::secrets::store::{self, SecretValue};

struct Component;

impl HttpGuest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request
            .get_path_with_query()
            .unwrap_or_else(|| "/".to_string());
        let (route, query) = match path.split_once('?') {
            Some((r, q)) => (r, q),
            None => (path.as_str(), ""),
        };

        if route.starts_with("/get") {
            let key = query_get(query, "key").unwrap_or_default();
            return match secret_string(&key).await {
                Some(value) => Ok(make_response(200, value.into_bytes())),
                None => Ok(make_response(404, Vec::new())),
            };
        }

        Ok(make_response(404, Vec::new()))
    }
}

/// Fetch `key` from the secrets plugin and reveal it as a UTF-8 string.
async fn secret_string(key: &str) -> Option<String> {
    let secret = store::get(key.to_string()).await.ok()?;
    match reveal(&secret).await {
        SecretValue::String(value) => Some(value),
        SecretValue::Bytes(bytes) => String::from_utf8(bytes).ok(),
    }
}

/// Return the value of `name` from a `k=v&k2=v2` query string, if present.
fn query_get(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

fn make_response(status: u16, body: Vec<u8>) -> Response {
    let headers = Fields::new();
    let _ = headers.set("content-type", &[b"application/octet-stream".to_vec()]);
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

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
