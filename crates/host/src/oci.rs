// Adapted from
// https://github.com/wasmCloud/wasmcloud-otp/blob/5f13500646d9e077afa1fca67a3fe9c8df5f3381/host_core/native/hostcore_wasmcloud_native/src/oci.rs

use crate::{par, RegistryConfig};

use core::str::FromStr;

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
use wasmcloud_core::tls;

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

impl From<crate::RegistryAuth> for RegistryAuth {
    fn from(auth: crate::RegistryAuth) -> Self {
        match auth {
            crate::RegistryAuth::Basic(username, password) => Self::Basic(username, password),
            _ => Self::Anonymous,
        }
    }
}

impl From<&crate::RegistryAuth> for RegistryAuth {
    fn from(auth: &crate::RegistryAuth) -> Self {
        match auth {
            crate::RegistryAuth::Basic(username, password) => {
                Self::Basic(username.clone(), password.clone())
            }
            _ => Self::Anonymous,
        }
    }
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

/// OCI artifact fetcher
#[derive(Clone, Debug)]
pub struct Fetcher {
    allow_latest: bool,
    allow_insecure: bool,
    auth: RegistryAuth,
}

impl Default for Fetcher {
    fn default() -> Self {
        Self {
            allow_latest: false,
            allow_insecure: false,
            auth: RegistryAuth::Anonymous,
        }
    }
}

impl From<&RegistryConfig> for Fetcher {
    fn from(
        RegistryConfig {
            auth,
            allow_latest,
            allow_insecure,
            ..
        }: &RegistryConfig,
    ) -> Self {
        Self {
            auth: auth.into(),
            allow_latest: *allow_latest,
            allow_insecure: *allow_insecure,
        }
    }
}

impl From<RegistryConfig> for Fetcher {
    fn from(
        RegistryConfig {
            auth,
            allow_latest,
            allow_insecure,
            ..
        }: RegistryConfig,
    ) -> Self {
        Self {
            auth: auth.into(),
            allow_latest,
            allow_insecure,
        }
    }
}

impl Fetcher {
    /// Fetch an OCI path
    async fn fetch_path(
        &self,
        img: impl AsRef<str>,
        accepted_media_types: Vec<&str>,
    ) -> anyhow::Result<PathBuf> {
        let img = img.as_ref();

        let img = &img.to_lowercase(); // the OCI spec does not allow for capital letters in references
        if !self.allow_latest && img.ends_with(":latest") {
            bail!("fetching images tagged 'latest' is currently prohibited in this host. This option can be overridden with WASMCLOUD_OCI_ALLOW_LATEST")
        }
        let cache_file = get_cached_filepath(img).await?;
        let digest_file = get_digest_filepath(img).await?;

        let img = Reference::from_str(img)?;

        let protocol = if self.allow_insecure {
            ClientProtocol::HttpsExcept(vec![img.registry().to_string()])
        } else {
            ClientProtocol::Https
        };
        let mut c = Client::new(ClientConfig {
            protocol,
            extra_root_certificates: tls::NATIVE_ROOTS_OCI.to_vec(),
            ..Default::default()
        });

        // In case of a cache miss where the file does not exist, pull a fresh OCI Image
        if fs::metadata(&cache_file).await.is_ok() {
            let (_, oci_digest) = c
                .pull_manifest(&img, &self.auth)
                .await
                .context("failed to fetch OCI manifest")?;
            // If the digest file doesn't exist that is ok, we just unwrap to an empty string
            let file_digest = fs::read_to_string(&digest_file).await.unwrap_or_default();
            if !oci_digest.is_empty() && !file_digest.is_empty() && file_digest == oci_digest {
                return Ok(cache_file);
            }
        }

        let imgdata = c
            .pull(&img, &self.auth, accepted_media_types)
            .await
            .context("failed to fetch OCI bytes")?;
        cache_oci_image(imgdata, &cache_file, digest_file)
            .await
            .context("failed to cache OCI bytes")?;
        Ok(cache_file)
    }

    /// Fetch actor from OCI
    ///
    /// # Errors
    ///
    /// Returns an error if either fetching fails or reading the fetched OCI path fails
    pub async fn fetch_actor(&self, oci_ref: impl AsRef<str>) -> anyhow::Result<Vec<u8>> {
        let path = self
            .fetch_path(oci_ref, vec![WASM_MEDIA_TYPE, OCI_MEDIA_TYPE])
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
    pub async fn fetch_provider(
        &self,
        oci_ref: impl AsRef<str>,
        host_id: impl AsRef<str>,
    ) -> anyhow::Result<(PathBuf, Option<jwt::Claims<jwt::CapabilityProvider>>)> {
        let path = self
            .fetch_path(
                oci_ref.as_ref(),
                vec![PROVIDER_ARCHIVE_MEDIA_TYPE, OCI_MEDIA_TYPE],
            )
            .await
            .context("failed to fetch OCI path")?;
        par::read(&path, host_id, oci_ref)
            .await
            .with_context(|| format!("failed to read `{}`", path.display()))
    }
}
