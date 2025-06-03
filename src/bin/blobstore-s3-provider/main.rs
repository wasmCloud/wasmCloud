use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_blobstore_s3::run()
        .await
        .context("failed to run provider")?;
    eprintln!("Blobstore S3 Provider exiting");
    Ok(())
}
