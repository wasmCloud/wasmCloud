//! Utilities for pulling and pushing artifacts to various registries

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context as _, Result};
use oci_distribution::manifest::OciImageManifest;
use oci_distribution::{
    client::{Client, ClientConfig, ClientProtocol, Config, ImageLayer},
    secrets::RegistryAuth,
    Reference,
};
use provider_archive::ProviderArchive;
use regex::RegexBuilder;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use wasmcloud_core::tls;

const PROVIDER_ARCHIVE_MEDIA_TYPE: &str = "application/vnd.wasmcloud.provider.archive.layer.v1+par";
const PROVIDER_ARCHIVE_CONFIG_MEDIA_TYPE: &str =
    "application/vnd.wasmcloud.provider.archive.config";
const WASM_MEDIA_TYPE: &str = "application/vnd.module.wasm.content.layer.v1+wasm";
const WASM_CONFIG_MEDIA_TYPE: &str = "application/vnd.wasmcloud.actor.archive.config";
const OCI_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";

// straight up stolen from oci_distribution::Reference
pub const REFERENCE_REGEXP: &str = r"^((?:(?:[a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9-]*[a-zA-Z0-9])(?:(?:\.(?:[a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9-]*[a-zA-Z0-9]))+)?(?::[0-9]+)?/)?[a-z0-9]+(?:(?:(?:[._]|__|[-]*)[a-z0-9]+)+)?(?:(?:/[a-z0-9]+(?:(?:(?:[._]|__|[-]*)[a-z0-9]+)+)?)+)?)(?::([\w][\w.-]{0,127}))?(?:@([A-Za-z][A-Za-z0-9]*(?:[-_+.][A-Za-z][A-Za-z0-9]*)*[:][[:xdigit:]]{32,}))?$";

/// Additional options for pulling an OCI artifact
#[derive(Default)]
pub struct OciPullOptions {
    /// The digest of the content you expect to receive. This is used for validation purposes only
    pub digest: Option<String>,
    /// By default, we do not allow latest tags in wasmCloud. This overrides that setting
    pub allow_latest: bool,
    /// An optional username to use for authentication.
    pub user: Option<String>,
    /// An optional password to use for authentication
    pub password: Option<String>,
    /// Whether or not to allow pulling from non-https registries
    pub insecure: bool,
}

/// Additional options for pushing an OCI artifact
#[derive(Default)]
pub struct OciPushOptions {
    /// A path to an optional OCI configuration
    pub config: Option<PathBuf>,
    /// By default, we do not allow latest tags in wasmCloud. This overrides that setting
    pub allow_latest: bool,
    /// An optional username to use for authentication.
    pub user: Option<String>,
    /// An optional password to use for authentication.
    pub password: Option<String>,
    /// Whether or not to allow pulling from non-https registries
    pub insecure: bool,
    /// Optional annotations you'd like to add to the pushed artifact
    pub annotations: Option<HashMap<String, String>>,
}

/// The types of artifacts that wash supports
pub enum SupportedArtifacts {
    /// A par.gz (i.e. parcheezy) file containing capability providers
    Par,
    /// WebAssembly modules
    Wasm,
}

// NOTE(thomastaylor312): In later refactors, we might want to consider making some sort of puller
// and pusher structs that can take optional implementations of a `Cache` trait that does all the
// cached file handling. But for now, this should be good enough

/// Attempts to return a local artifact, then a cached file (if `cache_file` is set).
///
/// Falls back to pull from registry if neither is found.
pub async fn get_oci_artifact(
    url_or_file: String,
    cache_file: Option<PathBuf>,
    options: OciPullOptions,
) -> Result<Vec<u8>> {
    if let Ok(mut local_artifact) = File::open(&url_or_file).await {
        let mut buf = Vec::new();
        local_artifact.read_to_end(&mut buf).await?;
        return Ok(buf);
    } else if let Some(cache_path) = cache_file {
        if let Ok(mut cached_artifact) = File::open(cache_path).await {
            let mut buf = Vec::new();
            cached_artifact.read_to_end(&mut buf).await?;
            return Ok(buf);
        }
    }
    pull_oci_artifact(url_or_file, options).await
}

/// Pull down the artifact from the given url and additional options
pub async fn pull_oci_artifact(url: String, options: OciPullOptions) -> Result<Vec<u8>> {
    let image: Reference = url.to_lowercase().parse()?;

    // NOTE(ceejimus): the FromStr implementation for the oci_distribution "Reference"
    // struct defaults the tag to "latest" if unspecified. Ideally, they would expose
    // some method to check if our valid input string has a tag or not, but, alas ...
    // earwax.
    // They don't even make the regex string public, so I stole it. These lines shouldn't fail
    // if the parsing for "image" doesn't. Unless of course the change it... what a pickle.
    let re = RegexBuilder::new(REFERENCE_REGEXP)
        .size_limit(10 * (1 << 21))
        .build()?;
    let input_tag = match re.captures(&url) {
        Some(caps) => caps.get(2).map(|m| m.as_str().to_owned()),
        None => bail!("Invalid OCI reference URL."),
    }
    .unwrap_or_default();

    if !options.allow_latest {
        if input_tag == "latest" {
            bail!("Pulling artifacts with tag 'latest' is prohibited. This can be overriden with the flag '--allow-latest'.");
        } else if input_tag.is_empty() {
            bail!("Registry URLs must have explicit tag. To default missing tags to 'latest', use the flag '--allow-latest'.");
        }
    }

    let mut client = Client::new(ClientConfig {
        protocol: if options.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        extra_root_certificates: tls::NATIVE_ROOTS_OCI.to_vec(),
        ..Default::default()
    });

    let auth = match (options.user, options.password) {
        (Some(user), Some(password)) => RegistryAuth::Basic(user, password),
        _ => RegistryAuth::Anonymous,
    };

    let image_data = client
        .pull(
            &image,
            &auth,
            vec![PROVIDER_ARCHIVE_MEDIA_TYPE, WASM_MEDIA_TYPE, OCI_MEDIA_TYPE],
        )
        .await?;

    // Reformatting digest in case the sha256: prefix is left off
    let digest = match options.digest {
        Some(d) if d.starts_with("sha256:") => Some(d),
        Some(d) => Some(format!("sha256:{d}")),
        None => None,
    };

    match (digest, image_data.digest) {
        (Some(digest), Some(image_digest)) if digest != image_digest => {
            bail!("image digest did not match provided digest, aborting")
        }
        _ => (),
    };

    Ok(image_data
        .layers
        .iter()
        .flat_map(|l| l.data.clone())
        .collect::<Vec<_>>())
}

pub async fn push_oci_artifact(
    url: String,
    artifact: impl AsRef<Path>,
    options: OciPushOptions,
) -> Result<()> {
    let image: Reference = url.to_lowercase().parse()?;

    if image.tag().unwrap() == "latest" && !options.allow_latest {
        bail!("Pushing artifacts with tag 'latest' is prohibited");
    };

    let mut artifact_buf = vec![];
    let mut f = File::open(&artifact)
        .await
        .with_context(|| format!("failed to open artifact [{}]", artifact.as_ref().display()))?;
    f.read_to_end(&mut artifact_buf).await?;

    let (artifact_media_type, config_media_type) = match validate_artifact(&artifact_buf).await? {
        SupportedArtifacts::Wasm => (WASM_MEDIA_TYPE, WASM_CONFIG_MEDIA_TYPE),
        SupportedArtifacts::Par => (
            PROVIDER_ARCHIVE_MEDIA_TYPE,
            PROVIDER_ARCHIVE_CONFIG_MEDIA_TYPE,
        ),
    };

    let mut config_buf = vec![];
    match options.config {
        Some(config_file) => {
            let mut f = File::open(&config_file).await.with_context(|| {
                format!("failed to open config file [{}]", config_file.display())
            })?;
            f.read_to_end(&mut config_buf).await?;
        }
        None => {
            // If no config provided, send blank config
            config_buf = b"{}".to_vec();
        }
    };
    let config = Config {
        data: config_buf,
        media_type: config_media_type.to_string(),
        annotations: None,
    };

    let layer = vec![ImageLayer {
        data: artifact_buf,
        media_type: artifact_media_type.to_string(),
        annotations: None,
    }];

    let mut client = Client::new(ClientConfig {
        protocol: if options.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        ..Default::default()
    });

    let auth = match (options.user, options.password) {
        (Some(user), Some(password)) => RegistryAuth::Basic(user, password),
        _ => RegistryAuth::Anonymous,
    };

    let manifest = OciImageManifest::build(&layer, &config, options.annotations);

    client
        .push(&image, &layer, config, &auth, Some(manifest))
        .await?;
    Ok(())
}

/// Helper function to determine artifact type and validate that it is
/// a supported artifact type
pub async fn validate_artifact(artifact: &[u8]) -> Result<SupportedArtifacts> {
    match validate_actor_module(artifact) {
        Ok(()) => Ok(SupportedArtifacts::Wasm),
        Err(_) => match validate_provider_archive(artifact).await {
            Ok(()) => Ok(SupportedArtifacts::Par),
            Err(_) => bail!("Unsupported artifact type"),
        },
    }
}

/// Attempts to inspect the claims of an actor module
/// Will fail without actor claims, or if the artifact is invalid
fn validate_actor_module(artifact: &[u8]) -> Result<()> {
    match wascap::wasm::extract_claims(artifact) {
        Ok(Some(_token)) => Ok(()),
        Ok(None) => bail!("No capabilities discovered in actor module"),
        Err(e) => bail!("{}", e),
    }
}

/// Attempts to unpack a provider archive
/// Will fail without claims or if the archive is invalid
async fn validate_provider_archive(artifact: &[u8]) -> Result<()> {
    match ProviderArchive::try_load(artifact).await {
        Ok(_par) => Ok(()),
        Err(e) => bail!("Invalid provider archive: {}", e),
    }
}
