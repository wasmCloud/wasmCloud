//! Workload component that makes an outbound `wasi:http` request to a
//! caller-supplied authority, so a test can point it at a local server and
//! observe how the host's egress transport behaves.
//!
//! - `GET /fetch?target=HOST:PORT` -> outbound `GET http://HOST:PORT/` ->
//!   200 with the upstream status in the body, or 502 with the error.

use wstd::http::{Body, Client, Request, Response, StatusCode};

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let path = req.uri().path_and_query().unwrap().as_str().to_string();
    let (route, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
    if route != "/fetch" {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("not found"))
            .unwrap());
    }
    let Some(target) = query_get(query, "target") else {
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("missing target"))
            .unwrap());
    };
    fetch(&format!("http://{target}/")).await
}

async fn fetch(url: &str) -> Result<Response<Body>, wstd::http::Error> {
    let client = Client::new();
    let req = Request::get(url).body(Body::empty()).unwrap();
    let (status, body) = match client.send(req).await {
        Ok(resp) => (StatusCode::OK, format!("upstream {}", resp.status())),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("failed: {e}")),
    };
    Ok(Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap())
}

/// Return the value of `name` from a `k=v&k2=v2` query string, if present.
fn query_get(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}
