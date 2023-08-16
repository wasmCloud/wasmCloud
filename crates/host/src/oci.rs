// Adapted from
// https://github.com/wasmCloud/wasmcloud-otp/blob/5f13500646d9e077afa1fca67a3fe9c8df5f3381/host_core/native/hostcore_wasmcloud_native/src/oci.rs

use crate::par;

use core::str::FromStr;

use std::collections::HashMap;
use std::env::temp_dir;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _};
use oci_distribution::client::{ClientConfig, ClientProtocol, ImageData};
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::{Client, Reference};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use wascap::jwt;

const PROVIDER_ARCHIVE_MEDIA_TYPE: &str = "application/vnd.wasmcloud.provider.archive.layer.v1+par";
const WASM_MEDIA_TYPE: &str = "application/vnd.module.wasm.content.layer.v1+wasm";
const OCI_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";

/// Configuration options for OCI operations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
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

fn determine_auth(
    image_reference: &str,
    creds_override: Option<HashMap<String, String>>,
    oci_config: &Config,
) -> RegistryAuth {
    match (creds_override, oci_config) {
        (
            _,
            Config {
                oci_registry: Some(reg),
                oci_user: Some(u),
                oci_password: Some(p),
                ..
            },
        ) if image_reference.starts_with(reg) => RegistryAuth::Basic(u.clone(), p.clone()),
        (Some(hm), _) => match (hm.get("username"), hm.get("password")) {
            (Some(un), Some(pw)) => RegistryAuth::Basic(un.to_string(), pw.to_string()),
            _ => RegistryAuth::Anonymous,
        },
        _ => RegistryAuth::Anonymous,
    }
}

/// Fetch an OCI path
#[allow(clippy::implicit_hasher)]
#[allow(clippy::missing_errors_doc)] // TODO: document errors
pub async fn fetch_oci_path(
    img: &str,
    creds_override: Option<HashMap<String, String>>,
    oci_config: &Config,
    accepted_media_types: Vec<&str>,
) -> anyhow::Result<PathBuf> {
    let img = &img.to_lowercase(); // the OCI spec does not allow for capital letters in references
    if !oci_config.allow_latest && img.ends_with(":latest") {
        bail!("fetching images tagged 'latest' is currently prohibited in this host. This option can be overridden with WASMCLOUD_OCI_ALLOW_LATEST")
    }
    let cache_file = get_cached_filepath(img).await?;
    let digest_file = get_digest_filepath(img).await?;

    let auth = determine_auth(img, creds_override, oci_config);
    let img = Reference::from_str(img)?;

    let protocol = ClientProtocol::HttpsExcept(oci_config.allowed_insecure.clone());
    let config = ClientConfig {
        protocol,
        ..Default::default()
    };
    let mut c = Client::new(config);

    // In case of a cache miss where the file does not exist, pull a fresh OCI Image
    if fs::metadata(&cache_file).await.is_ok() {
        let (_, oci_digest) = c
            .pull_manifest(&img, &auth)
            .await
            .context("failed to fetch OCI manifest")?;
        // If the digest file doesn't exist that is ok, we just unwrap to an empty string
        let file_digest = fs::read_to_string(&digest_file).await.unwrap_or_default();
        if !oci_digest.is_empty() && !file_digest.is_empty() && file_digest == oci_digest {
            return Ok(cache_file);
        }
    }

    let imgdata = c
        .pull(&img, &auth, accepted_media_types)
        .await
        .context("failed to fetch OCI bytes")?;
    cache_oci_image(imgdata, &cache_file, digest_file)
        .await
        .context("failed to cache OCI bytes")?;
    Ok(cache_file)
}

async fn get_cached_filepath(img: &str) -> std::io::Result<PathBuf> {
    let mut path = create_filepath(img).await?;
    path.set_extension("bin");

    Ok(path)
}

async fn get_digest_filepath(img: &str) -> std::io::Result<PathBuf> {
    let mut path = create_filepath(img).await?;
    path.set_extension("digest");

    Ok(path)
}

async fn create_filepath(img: &str) -> std::io::Result<PathBuf> {
    let path = temp_dir();
    let path = path.join("wasmcloud_ocicache");
    fs::create_dir_all(&path).await?;
    // should produce a file like wasmcloud_azurecr_io_kvcounter_v1
    let img = img.replace(':', "_");
    let img = img.replace('/', "_");
    let img = img.replace('.', "_");
    Ok(path.join(img))
}

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

/// Fetch actor from OCI
///
/// # Errors
///
/// Returns an error if either fetching fails or reading the fetched OCI path fails
#[allow(clippy::implicit_hasher)]
pub async fn fetch_actor(
    creds_override: Option<HashMap<String, String>>,
    oci_ref: impl AsRef<str>,
    host_oci_opts: &Config,
) -> anyhow::Result<Vec<u8>> {
    let path = fetch_oci_path(
        oci_ref.as_ref(),
        creds_override,
        host_oci_opts,
        vec![WASM_MEDIA_TYPE, OCI_MEDIA_TYPE],
    )
    .await
    .context("failed to fetch OCI path")?;
    fs::read(&path)
        .await
        .with_context(|| format!("failed to read `{}`", path.display()))
}

/// Fetch provider from OCI
///
/// # Errors
///
/// Returns an error if either fetching fails or reading the fetched OCI path fails
#[allow(clippy::implicit_hasher)]
pub async fn fetch_provider(
    oci_ref: impl AsRef<str>,
    link_name: impl AsRef<str>,
    creds_override: Option<HashMap<String, String>>,
    host_oci_opts: &Config,
) -> anyhow::Result<(PathBuf, jwt::Claims<jwt::CapabilityProvider>)> {
    let path = fetch_oci_path(
        oci_ref.as_ref(),
        creds_override,
        host_oci_opts,
        vec![PROVIDER_ARCHIVE_MEDIA_TYPE, OCI_MEDIA_TYPE],
    )
    .await
    .context("failed to fetch OCI path")?;
    par::read(&path, link_name)
        .await
        .with_context(|| format!("failed to read `{}`", path.display()))
}
