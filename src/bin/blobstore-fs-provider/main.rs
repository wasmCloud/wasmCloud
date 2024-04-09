use anyhow::Context as _;
use wasmcloud_provider_blobstore_fs::FsProvider;
use wasmcloud_provider_sdk::interfaces::blobstore::run_blobstore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_blobstore(FsProvider::default(), "blobstore-fs-provider")
        .await
        .context("failed to run provider")?;
    eprintln!("Blobstore FS Provider exiting");
    Ok(())
}
