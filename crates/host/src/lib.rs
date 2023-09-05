//! wasmCloud host library

#![warn(clippy::pedantic)]
#![warn(missing_docs)]
#![forbid(clippy::unwrap_used)]

/// local host
pub mod local;

/// wasmbus host
pub mod wasmbus;

/// bindle artifact fetching
pub mod bindle;

/// OCI artifact fetching
pub mod oci;

/// wasmCloud policy service
pub mod policy;

/// Common registry types
pub mod registry;

/// Provider archive functionality
mod par;

pub use local::{Host as LocalHost, HostConfig as LocalHostConfig};
pub use oci::{Config as OciConfig, Fetcher as OciFetcher};
pub use registry::{Auth as RegistryAuth, Config as RegistryConfig, Type as RegistryType};
pub use wasmbus::{Host as WasmbusHost, HostConfig as WasmbusHostConfig};

pub use url;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, bail, ensure, Context as _};
use tokio::fs;
use tracing::{instrument, warn};
use url::Url;
use wascap::jwt;

#[cfg(unix)]
fn socket_pair() -> anyhow::Result<(tokio::net::UnixStream, tokio::net::UnixStream)> {
    tokio::net::UnixStream::pair().context("failed to create an unnamed unix socket pair")
}

#[cfg(windows)]
fn socket_pair() -> anyhow::Result<(tokio::io::DuplexStream, tokio::io::DuplexStream)> {
    Ok(tokio::io::duplex(8196))
}

enum ResourceRef<'a> {
    File(PathBuf),
    Bindle(&'a str),
    Oci(&'a str),
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
                        .map_err(|_| anyhow!("failed to convert `{url}` to a file path")),
                    "bindle" => s
                        .strip_prefix("bindle://")
                        .map(Self::Bindle)
                        .context("invalid Bindle reference"),
                    "oci" => s
                        .strip_prefix("oci://")
                        .map(Self::Oci)
                        .context("invalid OCI reference"),
                    // TODO: Support other schemes
                    scheme => bail!("unsupported scheme `{scheme}`"),
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
            ResourceRef::Bindle(s) | ResourceRef::Oci(s) => {
                let (l, _) = s.split_once('/')?;
                Some(l)
            }
        }
    }
}

/// Fetch an actor from a reference.
#[instrument(skip(actor_ref))]
pub async fn fetch_actor(
    actor_ref: impl AsRef<str>,
    allow_file_load: bool,
    registry_config: &HashMap<String, RegistryConfig>,
) -> anyhow::Result<Vec<u8>> {
    match ResourceRef::try_from(actor_ref.as_ref())? {
        ResourceRef::File(actor_ref) => {
            ensure!(
                allow_file_load,
                "unable to start actor from file, file loading is disabled"
            );
            fs::read(actor_ref).await.context("failed to read actor")
        }
        ref bindle_ref @ ResourceRef::Bindle(actor_ref) => bindle_ref
            .authority()
            .and_then(|authority| registry_config.get(authority))
            .map(bindle::Fetcher::from)
            .unwrap_or_default()
            .fetch_actor(actor_ref)
            .await
            .with_context(|| format!("failed to fetch actor under Bindle reference `{actor_ref}`")),
        ref oci_ref @ ResourceRef::Oci(actor_ref) => oci_ref
            .authority()
            .and_then(|authority| registry_config.get(authority))
            .map(oci::Fetcher::from)
            .unwrap_or_default()
            .fetch_actor(actor_ref)
            .await
            .with_context(|| format!("failed to fetch actor under OCI reference `{actor_ref}`")),
    }
}

/// Fetch a provider from a reference.
#[instrument(skip(provider_ref, link_name))]
pub async fn fetch_provider(
    provider_ref: impl AsRef<str>,
    link_name: impl AsRef<str>,
    allow_file_load: bool,
    registry_config: &HashMap<String, RegistryConfig>,
) -> anyhow::Result<(PathBuf, jwt::Claims<jwt::CapabilityProvider>)> {
    match ResourceRef::try_from(provider_ref.as_ref())? {
        ResourceRef::File(provider_ref) => {
            ensure!(
                allow_file_load,
                "unable to start provider from file, file loading is disabled"
            );
            par::read(provider_ref, link_name)
                .await
                .context("failed to read provider")
        }
        ref bindle_ref @ ResourceRef::Bindle(provider_ref) => bindle_ref
            .authority()
            .and_then(|authority| registry_config.get(authority))
            .map(bindle::Fetcher::from)
            .unwrap_or_default()
            .fetch_provider(&provider_ref, link_name)
            .await
            .with_context(|| {
                format!("failed to fetch provider under Bindle reference `{provider_ref}`")
            }),
        ref oci_ref @ ResourceRef::Oci(provider_ref) => oci_ref
            .authority()
            .and_then(|authority| registry_config.get(authority))
            .map(oci::Fetcher::from)
            .unwrap_or_default()
            .fetch_provider(&provider_ref, link_name)
            .await
            .with_context(|| {
                format!("failed to fetch provider under OCI reference `{provider_ref}`")
            }),
    }
}
