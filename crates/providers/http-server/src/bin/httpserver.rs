use anyhow::Context as _;
use wasmcloud_provider_httpserver::HttpServerProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let fut = wasmcloud_provider_sdk::run_provider_handler(
        HttpServerProvider::default(),
        "http-server-provider",
    )
    .await
    .context("failed to run provider")?;
    fut.await;
    eprintln!("HttpServer provider exiting");
    Ok(())
}
