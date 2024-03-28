use anyhow::Context as _;
use wasmcloud_provider_http_server::HttpServerProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let shutdown = wasmcloud_provider_sdk::run_provider_handler(
        HttpServerProvider::default(),
        "http-server-provider",
    )
    .await
    .context("failed to run provider")?;
    shutdown.await;
    eprintln!("HttpServer provider exiting");
    Ok(())
}
