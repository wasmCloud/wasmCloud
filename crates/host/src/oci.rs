// Adapted from
// https://github.com/wasmCloud/wasmcloud-otp/blob/5f13500646d9e077afa1fca67a3fe9c8df5f3381/host_core/native/hostcore_wasmcloud_native/src/oci.rs

use std::path::{Path, PathBuf};

use oci_distribution::client::ImageData;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Configuration options for OCI operations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    /// Additional CAs to include in the OCI client configuration
    pub additional_ca_paths: Vec<PathBuf>,
    /// Whether or not to allow downloading OCI artifacts with the tag `latest`
    pub allow_latest: bool,
    /// A list of OCI registries that are allowed to be accessed over HTTP
    pub allowed_insecure: Vec<String>,
    /// Used in tandem with `oci_user` and `oci_password` to override credentials for a specific OCI registry.
    pub oci_registry: Option<String>,
    /// Username for the OCI registry specified by `oci_registry`.
    pub oci_user: Option<String>,
    /// Password for the OCI registry specified by `oci_registry`.
    pub oci_password: Option<String>,
}

#[allow(unused)]
async fn cache_oci_image(
    image: ImageData,
    cache_filepath: impl AsRef<Path>,
    digest_filepath: impl AsRef<Path>,
) -> std::io::Result<()> {
    let mut cache_file = fs::File::create(cache_filepath).await?;
    let content = image
        .layers
        .into_iter()
        .flat_map(|l| l.data)
        .collect::<Vec<_>>();
    cache_file.write_all(&content).await?;
    cache_file.flush().await?;
    if let Some(digest) = image.digest {
        let mut digest_file = fs::File::create(digest_filepath).await?;
        digest_file.write_all(digest.as_bytes()).await?;
        digest_file.flush().await?;
    }
    Ok(())
}

// TODO: add this after uses of fetch_component/fetch_provider
// cache_oci_image(imgdata, &cache_file, digest_file)
//     .await
//     .context("failed to cache OCI bytes")?;
