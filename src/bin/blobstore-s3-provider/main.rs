use anyhow::Context as _;
use wasmcloud_provider_blobstore_s3::BlobstoreS3Provider;
use wasmcloud_provider_sdk::interfaces::blobstore::run_blobstore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_blobstore(BlobstoreS3Provider::default(), "blobstore-s3-provider")
        .await
        .context("failed to run provider")?;
    eprintln!("Blobstore S3 Provider exiting");
    Ok(())
}
