use anyhow::Context as _;
use wasmcloud_provider_http_client::{serve, HttpClientProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let fut =
        wasmcloud_provider_sdk::run_provider_handler(HttpClientProvider, "http-client-provider")
            .await
            .context("failed to run provider")?;
    serve(fut).await?;
    eprintln!("HttpClient provider exiting");
    Ok(())
}
