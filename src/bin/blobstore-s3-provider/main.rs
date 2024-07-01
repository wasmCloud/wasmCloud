use anyhow::Context as _;

use wasmcloud_provider_blobstore_s3::BlobstoreS3Provider;
use wasmcloud_provider_sdk::initialize_observability;
use wasmcloud_provider_sdk::interfaces::blobstore::run_blobstore;

const PROVIDER_NAME: &str = "blobstore-s3-provider";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_observability!(
        PROVIDER_NAME,
        std::env::var_os("PROVIDER_BLOBSTORE_S3_FLAMEGRAPH_PATH")
    );

    run_blobstore(BlobstoreS3Provider::default(), PROVIDER_NAME)
        .await
        .context("failed to run provider")?;
    eprintln!("Blobstore S3 Provider exiting");
    Ok(())
}
