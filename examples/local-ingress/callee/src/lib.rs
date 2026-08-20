use wstd::http::error::Context;
use wstd::http::{Body, Request, Response, StatusCode};

/// Answers every request with a greeting that echoes the path and authority
/// it arrived with (WASI folds the `Host` header into the request authority).
/// When a co-located caller reaches this component through same-host local
/// routing, the echoed host is the authority the caller dialed (e.g.
/// `callee.internal`) — a hostname that resolves nowhere on the real network.
#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let host = req
        .uri()
        .authority()
        .map(|authority| authority.as_str())
        .unwrap_or("<none>");
    let body = format!(
        "hello from the callee! (path: {}, host: {})\n",
        req.uri().path(),
        host
    );
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(body))
        .context("building response")
}
