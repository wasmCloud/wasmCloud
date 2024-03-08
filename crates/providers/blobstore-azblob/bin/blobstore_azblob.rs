use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk;

use wasmcloud_provider_blobstore_azblob::BlobstoreAzblobProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // start_provider initializes the threaded tokio executor,
    // listens to lattice rpcs, handles actor links,
    // and returns only when it receives a shutdown message
    wasmcloud_provider_sdk::start_provider(
        BlobstoreAzblobProvider::default(),
        Some("blobstore-azblob-provider".to_string()),
    )?;

    eprintln!("Blobstore Azblob Provider exiting");
    Ok(())
}
