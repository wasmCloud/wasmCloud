use anyhow::Context;
use axum::{http, Router};
use hyper::server::conn::Http;
use wasmcloud_actor::{info, StdioStream};

async fn handle(method: http::Method, body: String) -> String {
    let res = format!(
        "[{}] received an HTTP {method} request with body: `{body}`",
        env!("CARGO_PKG_NAME")
    );
    info!("{}", res);
    res
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let router = Router::new().fallback(handle);
    Http::new()
        .http1_half_close(true)
        .http1_keep_alive(false)
        .serve_connection(StdioStream::default(), router)
        .await
        .context("failed to handle connection")
}
