//! Workload component driving the `tls-plugin-p3` host component plugin's
//! `acme:tlsprobe/probe` capability over HTTP.
//!
//! - `GET /ping?addr=A&name=N&via=wasi|wasmcloud` -> 200 body=`probe.ping(..)`

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "caller", generate_all });
}

use bindings::acme::tlsprobe::probe;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

impl HttpGuest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request
            .get_path_with_query()
            .unwrap_or_else(|| "/".to_string());
        let (route, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
        if route != "/ping" {
            return Ok(make_response(404, Vec::new()));
        }
        let addr = query_get(query, "addr").unwrap_or_default();
        let name = query_get(query, "name").unwrap_or_default();
        let via_wasi = query_get(query, "via").as_deref() == Some("wasi");
        let reply = probe::ping(addr, name, via_wasi).await;
        Ok(make_response(200, reply.into_bytes()))
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
