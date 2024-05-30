use anyhow::Context as _;
use wasmcloud_provider_http_client::HttpClientProvider;
use wasmcloud_provider_sdk::{interfaces::http::run_outgoing_handler, load_host_data};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let host_data = load_host_data()?;
    run_outgoing_handler(
        HttpClientProvider::new(&host_data.config).await?,
        "http-client-provider",
    )
    .await
    .context("failed to run provider")?;
    eprintln!("HttpClient provider exiting");
    Ok(())
}
