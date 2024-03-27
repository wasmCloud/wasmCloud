use anyhow::Context as _;
use wasmcloud_provider_blobstore_fs::FsProvider;
use wasmcloud_provider_sdk::interfaces::blobstore::serve_blobstore;
use wasmcloud_provider_sdk::run_provider_handler;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = FsProvider::default();
    let fut = run_provider_handler(provider.clone(), "blobstore-fs-provider")
        .await
        .context("failed to run provider")?;
    serve_blobstore(provider, fut).await?;
    eprintln!("Blobstore FS Provider exiting");
    Ok(())
}
