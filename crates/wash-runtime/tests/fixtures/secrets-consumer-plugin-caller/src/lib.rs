//! Workload component driving the imported `acme:secretsproxy/store`
//! capability over HTTP. The host resolves that import to the
//! `secrets-consumer-plugin` host component plugin running in its own store —
//! which in turn resolves ITS OWN `wasmcloud:secrets` import against the
//! host's native secrets plugin, never against another component plugin.
//!
//! - `GET /get?key=K` -> `store.get-secret(K)` -> 200 body=value, or 404

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "caller", generate_all });
}

use bindings::acme::secretsproxy::store;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

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

        if route.starts_with("/get-db-password") {
            return match store::get_db_password().await {
                Some(value) => Ok(make_response(200, value.into_bytes())),
                None => Ok(make_response(404, Vec::new())),
            };
        }

        if route.starts_with("/get") {
            let key = query_get(query, "key").unwrap_or_default();
            return match store::get_secret(key).await {
                Some(value) => Ok(make_response(200, value.into_bytes())),
                None => Ok(make_response(404, Vec::new())),
            };
        }

        Ok(make_response(404, Vec::new()))
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
    use super::{Component, bindings};
    bindings::export!(Component with_types_in bindings);
}
