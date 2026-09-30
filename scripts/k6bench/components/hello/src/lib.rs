//! Static 200, the same shape as the `http_invoke` criterion fixture: no host
//! calls, so a request measures the platform and nothing the guest does.

use wstd::http::error::Context;
use wstd::http::{Body, Request, Response};

#[wstd::http_server]
async fn main(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    Response::builder()
        .body(Body::from("hello from k6bench\n"))
        .context("building response")
}
