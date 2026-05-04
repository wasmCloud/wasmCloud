use anyhow::{Context, Result, anyhow};
use oci_client::{
    Reference,
    client::{Client, ClientConfig},
    secrets::RegistryAuth,
};
use oci_wasm::WASM_LAYER_MEDIA_TYPE;

pub async fn fetch(reference: &str) -> Result<Vec<u8>> {
    let parsed = Reference::try_from(reference)
        .with_context(|| format!("invalid OCI reference: {reference}"))?;

    let client = Client::new(ClientConfig::default());
    let auth = RegistryAuth::Anonymous;

    let image = client
        .pull(&parsed, &auth, vec![WASM_LAYER_MEDIA_TYPE])
        .await
        .with_context(|| format!("failed to pull {reference}"))?;

    // NOTE: Wasm OCI images contain a single layer
    // See: https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/
    let bytes = image
        .layers
        .first()
        .ok_or_else(|| anyhow!("no layers in pulled artifact: {reference}"))?
        .data
        .clone();

    Ok(bytes)
}
