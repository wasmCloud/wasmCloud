use anyhow::Context as _;
use wasmcloud_provider_http_client::HttpClientProvider;
use wasmcloud_provider_sdk::interfaces::http::serve_outgoing_handler;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = HttpClientProvider::default();
    let fut =
        wasmcloud_provider_sdk::run_provider_handler(provider.clone(), "http-client-provider")
            .await
            .context("failed to run provider")?;
    serve_outgoing_handler(provider, fut).await?;
    eprintln!("HttpClient provider exiting");
    Ok(())
}
