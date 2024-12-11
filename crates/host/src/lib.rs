//! wasmCloud host library

#![warn(missing_docs)]
#![forbid(clippy::unwrap_used)]

/// wasmbus host
pub mod wasmbus;

/// OCI artifact fetching
pub mod oci;

/// wasmCloud policy service
pub mod policy;

/// Common registry types
pub mod registry;

/// Secret management
pub mod secrets;

/// wasmCloud host metrics
pub(crate) mod metrics;

pub use metrics::HostMetrics;
pub use oci::Config as OciConfig;
pub use policy::{
    HostInfo as PolicyHostInfo, Manager as PolicyManager, Response as PolicyResponse,
};
pub use secrets::Manager as SecretsManager;
pub use wasmbus::{Host as WasmbusHost, HostConfig as WasmbusHostConfig};
pub use wasmcloud_core::{OciFetcher, RegistryAuth, RegistryConfig, RegistryType};

pub use url;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, bail, ensure, Context as _};
use tokio::fs;
use tracing::{debug, instrument, warn};
use url::Url;
use wascap::jwt;

#[derive(PartialEq)]
enum ResourceRef<'a> {
    File(PathBuf),
    Oci(&'a str),
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
#[instrument(level = "debug", skip(allow_file_load, registry_config))]
pub async fn fetch_component(
    component_ref: &str,
    allow_file_load: bool,
    additional_ca_paths: &Vec<PathBuf>,
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
            .unwrap_or_default()
            .with_additional_ca_paths(additional_ca_paths)
            .fetch_component(component_ref)
            .await
            .with_context(|| {
                format!("failed to fetch component under OCI reference `{component_ref}`")
            }),
        ResourceRef::Builtin(..) => bail!("nothing to fetch for a builtin"),
    }
}

/// Fetch a provider from a reference.
#[instrument(skip(registry_config, host_id), fields(provider_ref = %provider_ref.as_ref()))]
pub async fn fetch_provider(
    provider_ref: &ResourceRef<'_>,
    host_id: impl AsRef<str>,
    allow_file_load: bool,
    registry_config: &HashMap<String, RegistryConfig>,
) -> anyhow::Result<(PathBuf, Option<jwt::Token<jwt::CapabilityProvider>>)> {
    match provider_ref {
        ResourceRef::File(provider_path) => {
            ensure!(
                allow_file_load,
                "unable to start provider from file, file loading is disabled"
            );
            wasmcloud_core::par::read(
                provider_path,
                host_id,
                provider_ref,
                wasmcloud_core::par::UseParFileCache::Ignore,
            )
            .await
            .context("failed to read provider")
        }
        oci_ref @ ResourceRef::Oci(provider_ref) => oci_ref
            .authority()
            .and_then(|authority| registry_config.get(authority))
            .map(OciFetcher::from)
            .unwrap_or_default()
            .fetch_provider(provider_ref, host_id)
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
