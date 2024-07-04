use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_blobstore_azure::run()
        .await
        .context("failed to run provider")?;
    eprintln!("Blobstore Azure Provider exiting");
    Ok(())
}
