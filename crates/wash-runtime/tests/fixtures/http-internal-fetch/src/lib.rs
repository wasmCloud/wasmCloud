//! Makes an outgoing `wasi:http` request to whatever authority the request
//! path names, so a test can drive it at the reserved
//! `*.wasmcloud.internal` zone with a port it picked at runtime.
//!
//! `GET /<authority>` fetches `http://<authority>/`. The status reports what
//! the host's policy decided, not the upstream's:
//!
//! - `200` the request reached an upstream, whatever it answered
//! - `403` refused by the host (`allowedHosts`, or the host-loopback grant)
//! - `502` any other failure — no service listening, connection refused
//!
//! Separating 403 from 502 is the point: "the policy said no" and "there was
//! nothing there" have to be distinguishable, or a policy bug looks like a
//! missing service.

use wstd::http::error::ErrorCode;
use wstd::http::{Body, Client, Request, Response, StatusCode};

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let authority = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().trim_start_matches('/').to_string())
        .unwrap_or_default();
    if authority.is_empty() {
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("usage: GET /<authority>"))
            .unwrap());
    }
    fetch(&format!("http://{authority}/")).await
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

/// The host reports a policy refusal as `HttpRequestDenied`; everything else
/// is a transport failure.
fn status_for_error(e: &wstd::http::Error) -> StatusCode {
    match e.downcast_ref::<ErrorCode>() {
        Some(ErrorCode::HttpRequestDenied) => StatusCode::FORBIDDEN,
        // The reserved-zone paths report refusals as `InternalError` carrying
        // the reason, because they never reach the layer that would produce
        // `HttpRequestDenied`. Distinguish them by what the message says.
        Some(ErrorCode::InternalError(Some(message)))
            if message.contains("allowedHostLoopback")
                || message.contains("--allow-host-loopback")
                || message.contains("allowed_hosts") =>
        {
            StatusCode::FORBIDDEN
        }
        _ => StatusCode::BAD_GATEWAY,
    }
}
