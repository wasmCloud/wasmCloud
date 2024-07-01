use anyhow::Context as _;

use wasmcloud_provider_http_server::HttpServerProvider;
use wasmcloud_provider_sdk::initialize_observability;

const PROVIDER_NAME: &str = "http-server-provider";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_observability!(
        PROVIDER_NAME,
        std::env::var_os("PROVIDER_HTTP_SERVER_FLAMEGRAPH_PATH")
    );

    let shutdown =
        wasmcloud_provider_sdk::run_provider(HttpServerProvider::default(), PROVIDER_NAME)
            .await
            .context("failed to run provider")?;
    shutdown.await;
    eprintln!("HttpServer provider exiting");
    Ok(())
}
