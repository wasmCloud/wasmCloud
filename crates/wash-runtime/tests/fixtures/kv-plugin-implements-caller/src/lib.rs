//! Workload component driving ONE host component plugin through three imports
//! of the same interface: a plain one and two `(implements ..)` labels.
//!
//! - `GET /whichbinding?via=plain|tenant-a|tenant-b` -> `store.whichbinding()`
//!   through that import -> 200 body = the label the plugin read back, or
//!   `<none>` for a plain import.
//! - `GET /set?via=..&key=K&value=V` -> `store.set(K, V)` through that import.
//! - `GET /get?via=..&key=K`         -> `store.get(K)`, 200 body=V or 404.
//!
//! `set`/`get` are here so a test can show the labels reach one store rather
//! than three: what a label changes is the binding a call carries, not the data
//! behind it.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "implements-caller", generate_all });
}

use bindings::acme::kv::store;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::{tenant_a, tenant_b};

struct Component;

/// Which of the three imports a request names. Each is a distinct linker
/// instance, so the call has to be written against the matching bindings module
/// — there is no runtime dispatch that would let one import stand in for
/// another.
enum Via {
    Plain,
    TenantA,
    TenantB,
}

impl Via {
    fn parse(query: &str) -> Option<Self> {
        match query_get(query, "via").as_deref() {
            Some("plain") | None => Some(Self::Plain),
            Some("tenant-a") => Some(Self::TenantA),
            Some("tenant-b") => Some(Self::TenantB),
            Some(_) => None,
        }
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

        let Some(via) = Via::parse(query) else {
            return Ok(make_response(400, b"unknown `via`".to_vec()));
        };

        if route.starts_with("/whichbinding") {
            let binding = match via {
                Via::Plain => store::whichbinding().await,
                Via::TenantA => tenant_a::whichbinding().await,
                Via::TenantB => tenant_b::whichbinding().await,
            };
            // `<none>` rather than an empty body: a plain import genuinely has
            // no binding name, and a test asserting that must not be reading
            // the same bytes an empty string would give.
            let body = binding.unwrap_or_else(|| "<none>".to_string());
            return Ok(make_response(200, body.into_bytes()));
        }

        if route.starts_with("/set") {
            let key = query_get(query, "key").unwrap_or_default();
            let value = query_get(query, "value").unwrap_or_default().into_bytes();
            match via {
                Via::Plain => store::set(key, value).await,
                Via::TenantA => tenant_a::set(key, value).await,
                Via::TenantB => tenant_b::set(key, value).await,
            }
            return Ok(make_response(200, b"{\"ok\":true}".to_vec()));
        }

        if route.starts_with("/get") {
            let key = query_get(query, "key").unwrap_or_default();
            let found = match via {
                Via::Plain => store::get(key).await,
                Via::TenantA => tenant_a::get(key).await,
                Via::TenantB => tenant_b::get(key).await,
            };
            return match found {
                Some(v) => Ok(make_response(200, v)),
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
