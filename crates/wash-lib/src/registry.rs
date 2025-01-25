//! Utilities for pulling and pushing artifacts to various registries

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context as _, Result};
use oci_client::manifest::OciImageManifest;
use oci_client::{
    client::{Client, ClientConfig, ClientProtocol, Config, ImageLayer},
    secrets::RegistryAuth,
    Reference,
};
use oci_wasm::{ToConfig, WasmConfig, WASM_LAYER_MEDIA_TYPE, WASM_MANIFEST_MEDIA_TYPE};
use provider_archive::ProviderArchive;
use sha2::Digest;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use wasmcloud_core::tls;

const PROVIDER_ARCHIVE_MEDIA_TYPE: &str = "application/vnd.wasmcloud.provider.archive.layer.v1+par";
const PROVIDER_ARCHIVE_CONFIG_MEDIA_TYPE: &str =
    "application/vnd.wasmcloud.provider.archive.config";
const WASM_MEDIA_TYPE: &str = "application/vnd.module.wasm.content.layer.v1+wasm";
const OCI_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";

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
    /// Whether or not OCI registry's certificate will be checked for validity. This will make your HTTPS connections insecure.
    pub insecure_skip_tls_verify: bool,
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
    /// Whether or not OCI registry's certificate will be checked for validity. This will make your HTTPS connections insecure.
    pub insecure_skip_tls_verify: bool,
    /// Optional annotations you'd like to add to the pushed artifact
    pub annotations: Option<BTreeMap<String, String>>,
    /// Whether to use monolithic push instead of chunked push
    pub monolithic_push: bool,
}

/// The types of artifacts that wash supports
pub enum SupportedArtifacts {
    /// A par.gz (i.e. parcheezy) file containing capability providers
    Par(Config, ImageLayer),
    /// WebAssembly components and its configuration
    Wasm(Config, ImageLayer),
}

/// An enum indicating the type of artifact that was pulled
pub enum ArtifactType {
    Par,
    Wasm,
}

// Based on https://github.com/krustlet/oci-distribution/blob/v0.9.4/src/lib.rs#L25-L28
// We use this to calculate the sha256 digest for a given manifest so that we can return it
// back when pushing an artifact to a registry without making a network request for it.
/// Computes the SHA256 digest of a byte vector
fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", sha2::Sha256::digest(bytes))
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
    pull_oci_artifact(
        &url_or_file
            .try_into()
            .context("Unable to parse URL as a reference")?,
        options,
    )
    .await
}

/// Pull down the artifact from the given url and additional options
pub async fn pull_oci_artifact(image_ref: &Reference, options: OciPullOptions) -> Result<Vec<u8>> {
    let input_tag = image_ref.tag();

    if !options.allow_latest {
        if let Some(tag) = input_tag {
            if tag == "latest" {
                bail!("Pulling artifacts with tag 'latest' is prohibited. This can be overridden with the flag '--allow-latest'.");
            }
        } else {
            bail!("Registry URLs must have explicit tag. To default missing tags to 'latest', use the flag '--allow-latest'.");
        }
    }

    let client = Client::new(ClientConfig {
        protocol: if options.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        extra_root_certificates: tls::NATIVE_ROOTS_OCI.to_vec(),
        accept_invalid_certificates: options.insecure_skip_tls_verify,
        ..Default::default()
    });

    let auth = match (options.user, options.password) {
        (Some(user), Some(password)) => RegistryAuth::Basic(user, password),
        _ => RegistryAuth::Anonymous,
    };

    let image_data = client
        .pull(
            image_ref,
            &auth,
            vec![
                PROVIDER_ARCHIVE_MEDIA_TYPE,
                WASM_MEDIA_TYPE,
                OCI_MEDIA_TYPE,
                WASM_LAYER_MEDIA_TYPE,
            ],
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

/// Pushes the artifact to the given repo and returns a tuple containing the tag (if one was set) and the digest
pub async fn push_oci_artifact(
    url: String,
    artifact: impl AsRef<Path>,
    options: OciPushOptions,
) -> Result<(Option<String>, String)> {
    let image: Reference = url.to_lowercase().parse()?;

    if image.tag().unwrap_or_default() == "latest" && !options.allow_latest {
        bail!("Pushing artifacts with tag 'latest' is prohibited");
    };

    let mut artifact_buf = vec![];
    let mut f = File::open(&artifact)
        .await
        .with_context(|| format!("failed to open artifact [{}]", artifact.as_ref().display()))?;
    f.read_to_end(&mut artifact_buf).await?;

    let (config, layer, is_wasm) = match parse_and_validate_artifact(&artifact_buf).await? {
        SupportedArtifacts::Wasm(conf, layer) => (conf, layer, true),
        SupportedArtifacts::Par(mut conf, layer) => {
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
            conf.data = config_buf;
            (conf, layer, false)
        }
    };

    let layers = vec![layer];

    let client = Client::new(ClientConfig {
        protocol: if options.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        extra_root_certificates: tls::NATIVE_ROOTS_OCI.to_vec(),
        accept_invalid_certificates: options.insecure_skip_tls_verify,
        use_monolithic_push: options.monolithic_push,
        ..Default::default()
    });

    let auth = match (options.user, options.password) {
        (Some(user), Some(password)) => RegistryAuth::Basic(user, password),
        _ => RegistryAuth::Anonymous,
    };

    let mut manifest = OciImageManifest::build(&layers, &config, options.annotations);
    if is_wasm {
        manifest.media_type = Some(WASM_MANIFEST_MEDIA_TYPE.to_string());
    }
    // We calculate the sha256 digest from serde_json::Value instead of the OciImageManifest struct, because
    // when you serialize a struct directly into json, serde_json preserves the ordering of the keys.
    //
    // However, the registry implementations that were tested against (mostly based on distribution[1] registry),
    // all sort their manifests alphabetically (since they are based on Go), which means that when you calculate
    // the sha256-based digest, the ordering matters, and thus preserving the struct field ordering in the json
    // output causes the digests to differ.
    //
    // This attempts to approximate the ordering as provided by the Go-based registry implementations, which
    // is/are the prevailing implementation.
    let digest =
        serde_json::to_value(&manifest).map(|value| sha256_digest(value.to_string().as_bytes()))?;

    client
        .push(&image, &layers, config, &auth, Some(manifest))
        .await?;
    Ok((image.tag().map(ToString::to_string), digest))
}

/// Helper function to determine artifact type and parse it into a config and layer ready for use in
/// pushing to OCI
pub async fn parse_and_validate_artifact(artifact: &[u8]) -> Result<SupportedArtifacts> {
    // NOTE(thomastaylor312): I don't like having to clone here, but we need to either clone here or
    // later when calling parse_component/parse_provider_archive. If this gets to be a
    // problem, we can always change this, but it is a CLI, so _shrug_
    match parse_component(artifact.to_owned()) {
        Ok(art) => Ok(art),
        Err(_) => match parse_provider_archive(artifact).await {
            Ok(art) => Ok(art),
            Err(_) => bail!("Unsupported artifact type"),
        },
    }
}

/// Function that identifies whether the artifact is a component or a provider archive. Returns an
/// error if it isn't a known type
// NOTE: This exists because we don't care about parsing the proper world when pulling
pub async fn identify_artifact(artifact: &[u8]) -> Result<ArtifactType> {
    if wasmparser::Parser::is_component(artifact) {
        return Ok(ArtifactType::Wasm);
    }
    parse_provider_archive(artifact)
        .await
        .map(|_| ArtifactType::Par)
}

/// Attempts to parse the wit from a component. Fails if it isn't a component
fn parse_component(artifact: Vec<u8>) -> Result<SupportedArtifacts> {
    let (conf, layer) = WasmConfig::from_raw_component(artifact, None)?;
    Ok(SupportedArtifacts::Wasm(conf.to_config()?, layer))
}

/// Attempts to unpack a provider archive. Will fail without claims or if the archive is invalid
async fn parse_provider_archive(artifact: &[u8]) -> Result<SupportedArtifacts> {
    match ProviderArchive::try_load(artifact).await {
        Ok(_par) => Ok(SupportedArtifacts::Par(
            Config {
                data: Vec::default(),
                media_type: PROVIDER_ARCHIVE_CONFIG_MEDIA_TYPE.to_string(),
                annotations: None,
            },
            ImageLayer {
                data: artifact.to_owned(),
                media_type: PROVIDER_ARCHIVE_MEDIA_TYPE.to_string(),
                annotations: None,
            },
        )),
        Err(e) => bail!("Invalid provider archive: {}", e),
    }
}
