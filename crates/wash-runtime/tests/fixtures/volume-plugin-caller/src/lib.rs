//! Workload component driving the imported `acme:volume/files` capability over
//! HTTP. The host resolves that import to the `volume-plugin` host component
//! plugin running in its own store.
//!
//! - `GET /read?path=P` -> `files.read(P)` -> 200 body=the plugin's outcome
//! - `GET /write?path=P&contents=C` -> `files.write(P, C)` -> 200 body=outcome

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "caller", generate_all });
}

use bindings::acme::volume::files;
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
        let file = query_get(query, "path").unwrap_or_default();

        let outcome = if route.starts_with("/read") {
            files::read(file).await
        } else if route.starts_with("/write") {
            let contents = query_get(query, "contents").unwrap_or_default();
            files::write(file, contents).await
        } else {
            return Ok(make_response(404, Vec::new()));
        };
        Ok(make_response(200, outcome.into_bytes()))
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
