use anyhow::Context as _;

use wasmcloud_provider_blobstore_azure::BlobstoreAzblobProvider;
use wasmcloud_provider_sdk::initialize_observability;
use wasmcloud_provider_sdk::interfaces::blobstore::run_blobstore;

const PROVIDER_NAME: &str = "blobstore-azure-provider";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_observability!(
        PROVIDER_NAME,
        std::env::var_os("PROVIDER_BLOBSTORE_AZURE_FLAMEGRAPH_PATH")
    );
    run_blobstore(BlobstoreAzblobProvider::default(), PROVIDER_NAME)
        .await
        .context("failed to run provider")?;

    eprintln!("Blobstore Azure Provider exiting");
    Ok(())
}
