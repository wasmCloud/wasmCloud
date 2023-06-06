use anyhow::Context;
use axum::routing::get;
use axum::Router;
use hyper::server::conn::Http;
use wasmcloud_actor::StdioStream;

// Adapted from https://docs.rs/axum/latest/axum/#example

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let router = Router::new().route("/", get(|| async { "Hello, World!" }));
    Http::new()
        .http1_half_close(true)
        .http1_keep_alive(false)
        .serve_connection(StdioStream::default(), router)
        .await
        .context("failed to handle connection")
}
