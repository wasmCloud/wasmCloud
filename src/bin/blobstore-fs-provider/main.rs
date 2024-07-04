use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_blobstore_fs::run()
        .await
        .context("failed to run provider")?;
    eprintln!("Blobstore FS Provider exiting");
    Ok(())
}
