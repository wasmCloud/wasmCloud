use anyhow::Context as _;

use wasmcloud_provider_blobstore_fs::FsProvider;
use wasmcloud_provider_sdk::initialize_observability;
use wasmcloud_provider_sdk::interfaces::blobstore::run_blobstore;

const PROVIDER_NAME: &str = "blobstore-fs-provider";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_observability!(
        PROVIDER_NAME,
        std::env::var_os("PROVIDER_BLOBSTORE_FS_FLAMEGRAPH_PATH")
    );
    run_blobstore(FsProvider::default(), PROVIDER_NAME)
        .await
        .context("failed to run provider")?;
    eprintln!("Blobstore FS Provider exiting");
    Ok(())
}
