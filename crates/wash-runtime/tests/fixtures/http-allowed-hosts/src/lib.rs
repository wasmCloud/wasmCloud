use wstd::http::error::ErrorCode;
use wstd::http::{Body, Client, Request, Response, StatusCode};

// The fixture reports the host-side allowed_hosts policy decision via its
// own status code, not the upstream's:
//
// - 200 OK          — request reached upstream (which may return any status)
// - 403 Forbidden   — denied by the wasmCloud host's allowed_hosts policy
// - 502 Bad Gateway — any other client error (DNS, network, TLS, timeout)
//
// Reporting upstream status as-is would conflate "the host rejected this on
// policy grounds" with "the upstream returned a 4xx/5xx," so tests couldn't
// tell the two apart.

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match req.uri().path_and_query().unwrap().as_str() {
        "/example" => fetch("http://example.com").await,
        "/org" => fetch("http://example.org").await,
        "/www" => fetch("http://www.example.com").await,
        _ => not_found().await,
    }
}

async fn fetch(url: &str) -> Result<Response<Body>, wstd::http::Error> {
    let client = Client::new();
    let req = Request::get(url).body(Body::empty()).unwrap();
    let (status, body) = match client.send(req).await {
        Ok(resp) => (StatusCode::OK, format!("{url}: upstream {}", resp.status())),
        Err(e) => (status_for_error(&e), format!("{url} failed: {e}")),
    };
    Ok(Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap())
}

fn status_for_error(e: &wstd::http::Error) -> StatusCode {
    if matches!(
        e.downcast_ref::<ErrorCode>(),
        Some(ErrorCode::HttpRequestDenied)
    ) {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::BAD_GATEWAY
    }
}

async fn not_found() -> Result<Response<Body>, wstd::http::Error> {
    Ok(Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("not found"))
        .unwrap())
}
