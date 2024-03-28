use anyhow::Context as _;
use wasmcloud_provider_http_client::HttpClientProvider;
use wasmcloud_provider_sdk::interfaces::http::run_outgoing_handler;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_outgoing_handler(HttpClientProvider::default(), "http-client-provider")
        .await
        .context("failed to run provider")?;
    eprintln!("HttpClient provider exiting");
    Ok(())
}
