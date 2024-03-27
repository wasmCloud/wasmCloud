use anyhow::Context as _;
use wasmcloud_provider_blobstore_s3::BlobstoreS3Provider;
use wasmcloud_provider_sdk::interfaces::blobstore::serve_blobstore;
use wasmcloud_provider_sdk::run_provider_handler;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = BlobstoreS3Provider::default();
    let fut = run_provider_handler(provider.clone(), "blobstore-s3-provider")
        .await
        .context("failed to run provider")?;
    serve_blobstore(provider, fut).await?;
    eprintln!("Blobstore S3 Provider exiting");
    Ok(())
}
