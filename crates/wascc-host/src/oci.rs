use crate::Result;
use provider_archive::ProviderArchive;
use std::str::FromStr;

pub(crate) const OCI_VAR_USER: &str = "OCI_REGISTRY_USER";
pub(crate) const OCI_VAR_PASSWORD: &str = "OCI_REGISTRY_PASSWORD";

pub(crate) async fn fetch_oci_bytes(img: &str) -> Result<Vec<u8>> {
    let cfg = oci_distribution::client::ClientConfig::default();
    let mut c = oci_distribution::Client::new(cfg);

    let img = oci_distribution::Reference::from_str(img)?;
    let auth = if let Ok(u) = std::env::var(OCI_VAR_USER) {
        if let Ok(p) = std::env::var(OCI_VAR_PASSWORD) {
            oci_distribution::secrets::RegistryAuth::Basic(u, p)
        } else {
            oci_distribution::secrets::RegistryAuth::Anonymous
        }
    } else {
        oci_distribution::secrets::RegistryAuth::Anonymous
    };
    let imgdata: Result<oci_distribution::client::ImageData> = c
        .pull_image(&img, &auth)
        .await
        .map_err(|e| format!("{}", e).into());

    match imgdata {
        Ok(imgdata) => Ok(imgdata.content),
        Err(e) => {
            error!("Failed to fetch OCI bytes: {}", e);
            Err("Failed to fetch OCI bytes".into())
        }
    }
}

pub(crate) async fn fetch_provider_archive(img: &str) -> Result<ProviderArchive> {
    let bytes = fetch_oci_bytes(img).await?;
    ProviderArchive::try_load(&bytes)
        .map_err(|e| format!("Failed to load provider archive: {}", e).into())
}
