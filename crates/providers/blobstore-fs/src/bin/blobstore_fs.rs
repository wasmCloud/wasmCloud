use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk;

use wasmcloud_provider_blobstore_fs::FsProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // start_provider initializes the threaded tokio executor,
    // listens to lattice rpcs, handles actor links,
    // and returns only when it receives a shutdown message
    wasmcloud_provider_sdk::start_provider(FsProvider::default(), "blobstore-fs-provider")?;

    eprintln!("Blobstore FS Provider exiting");
    Ok(())
}
