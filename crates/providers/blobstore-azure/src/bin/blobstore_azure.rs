use anyhow::Context;
use wasmcloud_provider_sdk::run_provider_handler;

use wasmcloud_provider_blobstore_azure::BlobstoreAzblobProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = BlobstoreAzblobProvider::default();
    let fut = run_provider_handler(provider.clone(), "blobstore-azure-provider")
        .await
        .context("failed to run provider")?;
    provider.serve(fut).await?;

    eprintln!("Blobstore Azblob Provider exiting");
    Ok(())
}
