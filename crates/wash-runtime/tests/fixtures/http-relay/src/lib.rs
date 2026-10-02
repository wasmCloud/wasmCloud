//! Calls every URL in the comma-separated `TARGETS` config concurrently and
//! answers 200 only if all of them did. One target is a chain link, many are a
//! fan-out, none is a leaf.

use futures::future::join_all;
use wstd::http::error::Context;
use wstd::http::{Body, Client, Request, Response, StatusCode};

#[wstd::http_server]
async fn main(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let targets = std::env::var("TARGETS").unwrap_or_default();
    let targets: Vec<&str> = targets
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();

    let results = join_all(targets.iter().map(|target| call(target))).await;
    let failed: Vec<String> = results.into_iter().filter_map(Result::err).collect();

    let (status, body) = if failed.is_empty() {
        (
            StatusCode::OK,
            format!("relayed to {} targets\n", targets.len()),
        )
    } else {
        (StatusCode::BAD_GATEWAY, failed.join("\n") + "\n")
    };
    Response::builder()
        .status(status)
        .body(Body::from(body))
        .context("building response")
}

async fn call(target: &str) -> Result<(), String> {
    let req = Request::get(target)
        .body(Body::empty())
        .map_err(|e| format!("{target}: invalid request: {e}"))?;
    let mut resp = Client::new()
        .send(req)
        .await
        .map_err(|e| format!("{target}: {e}"))?;
    // Drain the body so the upstream response completes before we answer.
    let _ = resp.body_mut().contents().await;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("{target}: upstream {}", resp.status()))
    }
}
