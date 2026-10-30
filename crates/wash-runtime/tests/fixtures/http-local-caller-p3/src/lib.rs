//! P3 sibling of the p2 `http-allowed-hosts` fixture. Each route makes an
//! outgoing `wasi:http` request and reports what became of it as this
//! component's own status code, so a test reading the status can tell the three
//! outcomes apart:
//!
//! - 200 OK          — the outgoing request was served
//! - 403 Forbidden   — denied by the host's `allowed_hosts` policy
//! - 502 Bad Gateway — anything else (the stub network handler refuses, so this
//!   is what "it egressed" looks like)
//!
//! Reporting the upstream's own status instead would conflate "the host refused
//! this on policy grounds" with "the callee answered 4xx".

mod bindings {
    wit_bindgen::generate!({ world: "http-local-caller-p3", generate_all });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::client::send;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response, Scheme};

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        // The same targets the p2 fixture uses, so the two suites can assert
        // the same routing decisions against the same names.
        let (authority, path) = match request.get_path_with_query().unwrap_or_default().as_str() {
            "/example" => ("example.com", "/"),
            "/org" => ("example.org", "/"),
            // A non-root path, so a path-scoped `localRoute` is exercised. The
            // authority is deliberately not one of the two above.
            "/path" => ("gateway.test", "/functiona/items"),
            _ => return Ok(respond(404, b"not found".to_vec())),
        };

        let (status, body) = match fetch(authority, path).await {
            Ok(upstream) => (200, format!("{authority}{path}: upstream {upstream}")),
            Err(ErrorCode::HttpRequestDenied) => {
                (403, format!("{authority}{path}: denied by policy"))
            }
            Err(e) => (502, format!("{authority}{path} failed: {e:?}")),
        };
        Ok(respond(status, body.into_bytes()))
    }
}

/// One outgoing GET, returning the upstream status or the error code the host
/// answered with.
async fn fetch(authority: &str, path: &str) -> Result<u16, ErrorCode> {
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    // A GET with no body: the trailers future still has to resolve, or the
    // request side never completes.
    let (request, _sent) = Request::new(Fields::new(), None, trailers_rx, None);
    wit_bindgen::spawn_local(async move {
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let set = |r: Result<(), ()>| r.map_err(|()| ErrorCode::InternalError(None));
    set(request.set_scheme(Some(&Scheme::Http)))?;
    set(request.set_authority(Some(authority)))?;
    set(request.set_path_with_query(Some(path)))?;

    let response = send(request).await?;
    Ok(response.get_status_code())
}

fn respond(status: u16, body: Vec<u8>) -> Response {
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        tx.write_all(body).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
    // The status codes above are all valid, so this cannot fail.
    let _ = response.set_status_code(status);
    response
}

bindings::export!(Component with_types_in bindings);
