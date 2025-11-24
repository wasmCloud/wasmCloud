#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![forbid(clippy::unwrap_used)]

/// [crate::config::ConfigManager] trait for managing a config store which can be watched to receive
/// updates to the config. This is a supertrait of [crate::store::StoreManager] and is implemented
/// by [crate::store::DefaultStore].
pub mod config;

/// [crate::event::EventPublisher] trait for receiving and publishing events from the host
pub mod event;

/// NATS implementations of [crate::policy::PolicyManager], [crate::secrets::SecretsManager], and
/// [crate::store::StoreManager] traits for the wasmCloud host.
pub mod nats;

/// Implementation of OpenTelemetry metrics for wasmCloud, primarily using [wasmcloud_tracing]
pub mod metrics;

/// Configuration for OCI artifact fetching [crate::oci::Config]
pub mod oci;

/// [crate::policy::PolicyManager] trait for layering additional security policies on top of the
/// wasmCloud host
pub mod policy;

/// [crate::registry::RegistryCredentialExt] extension trait for converting registry credentials
/// into [wasmcloud_core::RegistryConfig]
pub mod registry;

/// [crate::secrets::SecretsManager] trait for fetching secrets from a secret store
pub mod secrets;

/// [crate::store::StoreManager] trait for fetching configuration and data from a backing store
pub mod store;

/// [crate::wasmbus::Host] implementation
pub mod wasmbus;

/// experimental workload identity implementation
pub mod workload_identity;

pub(crate) mod bindings {
    wit_bindgen_wrpc::generate!({ generate_all });
}

pub use oci::Config as OciConfig;
pub use policy::{HostInfo as PolicyHostInfo, PolicyManager, Response as PolicyResponse};
pub use wasmbus::{Host as WasmbusHost, HostConfig as WasmbusHostConfig};

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, bail, ensure, Context as _};
use tokio::fs;
use tracing::{debug, instrument, warn};
use url::Url;
use wascap::jwt;
use wasmcloud_core::{OciFetcher, RegistryAuth, RegistryConfig, RegistryType};

/// A reference to a resource, either a file, an OCI image, or a builtin provider
#[derive(PartialEq)]
pub enum ResourceRef<'a> {
    /// A file reference
    File(PathBuf),
    /// An OCI reference
    Oci(&'a str),
    /// A builtin provider reference
    Builtin(&'a str),
}

impl AsRef<str> for ResourceRef<'_> {
    fn as_ref(&self) -> &str {
        match self {
            // Resource ref must have originated from a URL, which can only be constructed from a
            // valid string
            ResourceRef::File(path) => path.to_str().expect("invalid file reference URL"),
            ResourceRef::Oci(s) => s,
            ResourceRef::Builtin(s) => s,
        }
    }
}

impl<'a> TryFrom<&'a str> for ResourceRef<'a> {
    type Error = anyhow::Error;

    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        match Url::parse(s) {
            Ok(url) => {
                match url.scheme() {
                    "file" => url
                        .to_file_path()
                        .map(Self::File)
                        .map_err(|()| anyhow!("failed to convert `{url}` to a file path")),
                    "oci" => {
                        // Note: oci is not a scheme, but using this as a prefix takes out the guesswork
                        s.strip_prefix("oci://")
                            .map(Self::Oci)
                            .context("invalid OCI reference")
                    }
                    "wasmcloud+builtin" => s
                        .strip_prefix("wasmcloud+builtin://")
                        .map(Self::Builtin)
                        .context("invalid builtin reference"),
                    scheme @ ("http" | "https") => {
                        debug!(%url, "interpreting reference as OCI");
                        s.strip_prefix(&format!("{scheme}://"))
                            .map(Self::Oci)
                            .context("invalid OCI reference")
                    }
                    _ => {
                        // handle strings like `registry:5000/v2/foo:0.1.0`
                        debug!(%url, "unknown scheme in reference, assuming OCI");
                        Ok(Self::Oci(s))
                    }
                }
            }
            Err(url::ParseError::RelativeUrlWithoutBase) => {
                match Url::parse(&format!("oci://{s}")) {
                    Ok(_url) => Ok(Self::Oci(s)),
                    Err(e) => Err(anyhow!(e).context("failed to parse reference as OCI reference")),
                }
            }
            Err(e) => {
                bail!(anyhow!(e).context(format!("failed to parse reference `{s}`")))
            }
        }
    }
}

impl ResourceRef<'_> {
    fn authority(&self) -> Option<&str> {
        match self {
            ResourceRef::File(_) => None,
            ResourceRef::Oci(s) => {
                let (l, _) = s.split_once('/')?;
                Some(l)
            }
            ResourceRef::Builtin(_) => None,
        }
    }
}

/// Fetch an component from a reference.
#[instrument(level = "debug", skip(default_config, registry_config))]
pub async fn fetch_component(
    component_ref: &str,
    allow_file_load: bool,
    default_config: &oci::Config,
    registry_config: &HashMap<String, RegistryConfig>,
) -> anyhow::Result<Vec<u8>> {
    match ResourceRef::try_from(component_ref)? {
        ResourceRef::File(component_ref) => {
            ensure!(
                allow_file_load,
                "unable to start component from file, file loading is disabled"
            );
            fs::read(component_ref)
                .await
                .context("failed to read component")
        }
        ref oci_ref @ ResourceRef::Oci(component_ref) => oci_ref
            .authority()
            .and_then(|authority| registry_config.get(authority))
            .map(OciFetcher::from)
            .unwrap_or_else(|| {
                OciFetcher::from(
                    RegistryConfig::builder()
                        .reg_type(RegistryType::Oci)
                        .additional_ca_paths(default_config.additional_ca_paths.clone())
                        .allow_latest(default_config.allow_latest)
                        .allow_insecure(
                            oci_ref
                                .authority()
                                .map(|authority| {
                                    default_config
                                        .allowed_insecure
                                        .contains(&authority.to_string())
                                })
                                .unwrap_or(false),
                        )
                        .auth(RegistryAuth::Anonymous)
                        .build()
                        .unwrap_or_default(),
                )
            })
            .with_additional_ca_paths(&default_config.additional_ca_paths)
            .fetch_component(component_ref)
            .await
            .with_context(|| {
                format!("failed to fetch component under OCI reference `{component_ref}`")
            }),
        ResourceRef::Builtin(..) => bail!("nothing to fetch for a builtin"),
    }
}

/// Fetch a provider from a reference.
#[instrument(skip(registry_config), fields(provider_ref = %provider_ref.as_ref()))]
pub(crate) async fn fetch_provider(
    provider_ref: &ResourceRef<'_>,
    allow_file_load: bool,
    default_config: &oci::Config,
    registry_config: &HashMap<String, RegistryConfig>,
) -> anyhow::Result<(PathBuf, Option<jwt::Token<jwt::CapabilityProvider>>)> {
    match provider_ref {
        ResourceRef::File(provider_path) => {
            ensure!(
                allow_file_load,
                "unable to start provider from file, file loading is disabled"
            );

            let source_path: &std::path::Path = provider_path.as_ref();

            ensure!(
                source_path.exists(),
                "Provider binary not found at path: {}",
                source_path.display()
            );

            ensure!(
                source_path.is_file(),
                "Provider path is not a file: {}",
                source_path.display()
            );

            // todo (luk3ark) no JWT provided...
            Ok((source_path.to_path_buf(), None))
        }
        oci_ref @ ResourceRef::Oci(provider_ref) => oci_ref
            .authority()
            .and_then(|authority| registry_config.get(authority))
            .map(OciFetcher::from)
            .unwrap_or_else(|| {
                OciFetcher::from(
                    RegistryConfig::builder()
                        .reg_type(RegistryType::Oci)
                        .additional_ca_paths(default_config.additional_ca_paths.clone())
                        .allow_latest(default_config.allow_latest)
                        .allow_insecure(
                            oci_ref
                                .authority()
                                .map(|authority| {
                                    default_config
                                        .allowed_insecure
                                        .contains(&authority.to_string())
                                })
                                .unwrap_or(false),
                        )
                        .auth(RegistryAuth::Anonymous)
                        .build()
                        .unwrap_or_default(),
                )
            })
            .with_additional_ca_paths(&default_config.additional_ca_paths)
            .fetch_provider(provider_ref)
            .await
            .with_context(|| {
                format!("failed to fetch provider under OCI reference `{provider_ref}`")
            }),
        ResourceRef::Builtin(..) => bail!("nothing to fetch for a builtin"),
    }
}

#[test]
fn parse_references() -> anyhow::Result<()> {
    // file:// URL
    let file_url = "file:///tmp/foo_s.wasm";
    ensure!(
        ResourceRef::try_from(file_url).expect("failed to parse")
            == ResourceRef::File("/tmp/foo_s.wasm".into()),
        "file reference should be parsed as file and converted to path"
    );

    // oci:// "scheme" URL
    ensure!(
        ResourceRef::try_from("oci://some-registry/foo:0.1.0").expect("failed to parse")
            == ResourceRef::Oci("some-registry/foo:0.1.0"),
        "OCI reference should be parsed as OCI and stripped of scheme"
    );

    // http URL
    ensure!(
        ResourceRef::try_from("http://127.0.0.1:5000/v2/foo:0.1.0").expect("failed to parse")
            == ResourceRef::Oci("127.0.0.1:5000/v2/foo:0.1.0"),
        "http reference should be parsed as OCI and stripped of scheme"
    );

    // https URL
    ensure!(
        ResourceRef::try_from("https://some-registry.sh/foo:0.1.0").expect("failed to parse")
            == ResourceRef::Oci("some-registry.sh/foo:0.1.0"),
        "https reference should be parsed as OCI and stripped of scheme"
    );

    // localhost URL
    ensure!(
        ResourceRef::try_from("localhost:5000/v2/foo:0.1.0").expect("failed to parse")
            == ResourceRef::Oci("localhost:5000/v2/foo:0.1.0"),
        "localhost reference should be parsed as OCI and left intact"
    );

    // container name URL
    ensure!(
        ResourceRef::try_from("registry:5000/v2/foo:0.1.0").expect("failed to parse")
            == ResourceRef::Oci("registry:5000/v2/foo:0.1.0"),
        "container reference should be parsed as OCI and left intact"
    );

    Ok(())
}
