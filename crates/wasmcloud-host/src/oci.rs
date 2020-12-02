use crate::Result;
use provider_archive::ProviderArchive;
use std::env::temp_dir;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::str::FromStr;

pub(crate) const OCI_VAR_USER: &str = "OCI_REGISTRY_USER";
pub(crate) const OCI_VAR_PASSWORD: &str = "OCI_REGISTRY_PASSWORD";

pub(crate) async fn fetch_oci_bytes(img: &str, allow_latest: bool) -> Result<Vec<u8>> {
    if !allow_latest && img.ends_with(":latest") {
        return Err(
            "Fetching images tagged 'latest' is currently prohibited in this host. This option can be overridden".into());
    }
    let cf = cached_file(img);
    if !cf.exists() {
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
            Ok(imgdata) => {
                let mut f = std::fs::File::create(cf)?;
                f.write_all(&imgdata.content)?;
                f.flush()?;
                Ok(imgdata.content)
            }
            Err(e) => {
                error!("Failed to fetch OCI bytes: {}", e);
                Err("Failed to fetch OCI bytes".into())
            }
        }
    } else {
        let mut buf = vec![];
        let mut f = std::fs::File::open(cached_file(img))?;
        f.read_to_end(&mut buf);
        Ok(buf)
    }
}

fn cached_file(img: &str) -> PathBuf {
    let path = temp_dir();
    let path = path.join("wasmcloud_ocicache");
    let _ = ::std::fs::create_dir_all(&path);
    // should produce a file like wascc_azurecr_io_kvcounter_v1.bin
    let img = img.replace(":", "_");
    let img = img.replace("/", "_");
    let img = img.replace(".", "_");
    let mut path = path.join(img);
    path.set_extension("bin");

    path
}

pub(crate) async fn fetch_provider_archive(
    img: &str,
    allow_latest: bool,
) -> Result<ProviderArchive> {
    let bytes = fetch_oci_bytes(img, allow_latest).await?;
    ProviderArchive::try_load(&bytes)
        .map_err(|e| format!("Failed to load provider archive: {}", e).into())
}
