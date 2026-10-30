use wstd::http::error::Context;
use wstd::http::{Body, Client, Request, Response, StatusCode};

/// Calls the callee over `wasi:http/outgoing-handler` and reports what
/// happened. The target URL is resolved in order from:
///
/// 1. a `?url=<target>` query parameter,
/// 2. the `CALLEE_URL` environment variable,
/// 3. `http://callee.internal/hello` — a hostname that resolves nowhere on
///    the real network, so the call only succeeds when the host serves it
///    via same-host local routing (`--http-local-routing` on the host, plus a
///    co-located callee whose `wasi:http/incoming-handler` config registers
///    that authority in `localRoute` with "callee.internal/hello" as the path).
#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let target = req
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("url=")))
        .map(str::to_string)
        .or_else(|| std::env::var("CALLEE_URL").ok())
        .unwrap_or_else(|| "http://callee.internal/hello".to_string());

    let outgoing = match Request::get(&target).body(Body::empty()) {
        Ok(outgoing) => outgoing,
        Err(e) => {
            return respond(
                StatusCode::BAD_REQUEST,
                format!("invalid target url {target}: {e}\n"),
            );
        }
    };

    match Client::new().send(outgoing).await {
        Ok(mut resp) => {
            let status = resp.status();
            let upstream = match resp.body_mut().str_contents().await {
                Ok(contents) => contents.to_string(),
                Err(e) => format!("<unreadable body: {e}>"),
            };
            respond(
                StatusCode::OK,
                format!("caller -> {target}\nupstream status: {status}\nupstream body: {upstream}"),
            )
        }
        Err(e) => respond(
            StatusCode::BAD_GATEWAY,
            format!("caller -> {target}\nrequest failed: {e}\n"),
        ),
    }
}

fn respond(status: StatusCode, body: String) -> Result<Response<Body>, wstd::http::Error> {
    Response::builder()
        .status(status)
        .body(Body::from(body))
        .context("building response")
}
