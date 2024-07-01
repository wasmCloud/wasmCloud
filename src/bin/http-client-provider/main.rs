use anyhow::Context as _;

use wasmcloud_provider_http_client::HttpClientProvider;
use wasmcloud_provider_sdk::initialize_observability;
use wasmcloud_provider_sdk::{interfaces::http::run_outgoing_handler, load_host_data};

const PROVIDER_NAME: &str = "http-client-provider";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_observability!(
        PROVIDER_NAME,
        std::env::var_os("PROVIDER_HTTP_CLIENT_FLAMEGRAPH_PATH")
    );
    let host_data = load_host_data()?;
    run_outgoing_handler(
        HttpClientProvider::new(&host_data.config).await?,
        PROVIDER_NAME,
    )
    .await
    .context("failed to run provider")?;
    eprintln!("HttpClient provider exiting");
    Ok(())
}
