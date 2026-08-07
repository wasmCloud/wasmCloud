//! Workload component sitting on both sides of `acme:events`: it imports
//! `control` (served by the host component plugin, the usual capability
//! direction) and exports `handler` (which the plugin calls back into, the
//! direction this fixture exists to exercise).
//!
//! Every reply from `notify` is prefixed with this workload's `EVENT_TAG`, so a
//! test can tell WHICH workload actually ran the callback rather than only that
//! one did:
//!
//! - `GET /echo?msg=M`           -> `control.echo(M)`          -> handling tag
//! - `GET /dispatch?id=W&msg=M`  -> `control.dispatch(W, M)`   -> W's tag
//! - `GET /nested?id=W&msg=M`    -> `control.nested(W, M)`     -> `W|self`
//! - `GET /callable`             -> `control.callable()`       -> ids, one per line
//! - `GET /tag`                  -> this workload's own tag

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "caller", generate_all });
}

use bindings::acme::events::control;
use bindings::exports::acme::events::handler::Guest as HandlerGuest;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

/// This workload's own name, from its manifest environment. Lets two deploys of
/// this same wasm be told apart in a callback's reply.
fn tag() -> String {
    std::env::var("EVENT_TAG").unwrap_or_else(|_| "untagged".to_string())
}

impl HandlerGuest for Component {
    /// Called by the plugin across the store boundary. Reports which workload
    /// handled it, and what it was handed.
    async fn notify(message: String) -> String {
        format!("{}:{message}", tag())
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

        if route.starts_with("/echo") {
            let msg = query_get(query, "msg").unwrap_or_default();
            let reply = control::echo(msg).await;
            return Ok(make_response(200, reply.into_bytes()));
        }
        if route.starts_with("/dispatch") {
            let id = query_get(query, "id").unwrap_or_default();
            let msg = query_get(query, "msg").unwrap_or_default();
            let reply = control::dispatch(id, msg).await;
            return Ok(make_response(200, reply.into_bytes()));
        }
        if route.starts_with("/nested") {
            let id = query_get(query, "id").unwrap_or_default();
            let msg = query_get(query, "msg").unwrap_or_default();
            let reply = control::nested(id, msg).await;
            return Ok(make_response(200, reply.into_bytes()));
        }
        if route.starts_with("/callable") {
            let ids = control::callable().await;
            return Ok(make_response(200, ids.join("\n").into_bytes()));
        }
        if route.starts_with("/tag") {
            return Ok(make_response(200, tag().into_bytes()));
        }

        Ok(make_response(404, Vec::new()))
    }
}

fn query_get(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

fn make_response(status: u16, body: Vec<u8>) -> Response {
    let headers = Fields::new();
    let _ = headers.set(
        &"content-type".to_string(),
        &[b"application/octet-stream".to_vec()],
    );
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
