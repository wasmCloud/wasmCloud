//! Wasmcloud Wadm Provider

use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_wadm::run()
        .await
        .context("failed to run provider")?;
    eprintln!(" provider exiting");
    Ok(())
}
