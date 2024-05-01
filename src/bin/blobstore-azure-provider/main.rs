use anyhow::Context as _;
use wasmcloud_provider_blobstore_azure::BlobstoreAzblobProvider;
use wasmcloud_provider_sdk::interfaces::blobstore::run_blobstore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_blobstore(BlobstoreAzblobProvider::default(), "blobstore-fs-provider")
        .await
        .context("failed to run provider")?;

    eprintln!("Blobstore Azblob Provider exiting");
    Ok(())
}
