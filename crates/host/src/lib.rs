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

/// Provider archive functionality
mod par;

/// Common registry types
mod registry;

pub use local::{Host as LocalHost, HostConfig as LocalHostConfig};
use registry::Settings as RegistrySettings;
pub use wasmbus::{Host as WasmbusHost, HostConfig as WasmbusHostConfig};

pub use url;

use std::{collections::HashMap, path::PathBuf};

use anyhow::{anyhow, bail, Context as _};
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

fn extract_server<'a>(resource_ref: &ResourceRef<'a>) -> Option<&'a str> {
    match resource_ref {
        ResourceRef::File(_) => None,
        ResourceRef::Bindle(resource_ref) | ResourceRef::Oci(resource_ref) => {
            resource_ref.split('/').next()
        }
    }
}

/// Fetch an actor from a reference.
#[instrument(skip(actor_ref))]
pub async fn fetch_actor(
    actor_ref: impl AsRef<str>,
    allow_file_load: bool,
    all_registry_settings: &HashMap<String, RegistrySettings>,
) -> anyhow::Result<Vec<u8>> {
    let actor_ref = ResourceRef::try_from(actor_ref.as_ref())?;
    let server = extract_server(&actor_ref).unwrap_or_default();
    let default_registry_settings = RegistrySettings::default();
    let registry_settings = all_registry_settings
        .get(server)
        .unwrap_or(&default_registry_settings);

    match actor_ref {
        ResourceRef::File(actor_ref) if allow_file_load => {
            fs::read(actor_ref).await.context("failed to read actor")
        }
        ResourceRef::File(_) => bail!("unable to start actor from file, file loading is disabled"),
        ResourceRef::Bindle(actor_ref) => crate::bindle::fetch_actor(actor_ref, registry_settings)
            .await
            .with_context(|| format!("failed to fetch actor under Bindle reference `{actor_ref}`")),
        ResourceRef::Oci(actor_ref) => crate::oci::fetch_actor(actor_ref, registry_settings)
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
    all_registry_settings: &HashMap<String, RegistrySettings>,
) -> anyhow::Result<(PathBuf, jwt::Claims<jwt::CapabilityProvider>)> {
    let provider_ref = ResourceRef::try_from(provider_ref.as_ref())?;
    let server = extract_server(&provider_ref).unwrap_or_default();
    let default_registry_settings = RegistrySettings::default();
    let registry_settings = all_registry_settings
        .get(server)
        .unwrap_or(&default_registry_settings);

    match provider_ref {
        ResourceRef::File(provider_ref) if allow_file_load => par::read(provider_ref, link_name)
            .await
            .context("failed to read provider"),
        ResourceRef::File(_) => {
            bail!("unable to start provider from file, file loading is disabled")
        }
        ResourceRef::Bindle(provider_ref) => {
            crate::bindle::fetch_provider(&provider_ref, link_name, registry_settings)
                .await
                .with_context(|| {
                    format!("failed to fetch provider under Bindle reference `{provider_ref}`")
                })
        }
        ResourceRef::Oci(provider_ref) => {
            crate::oci::fetch_provider(&provider_ref, link_name, registry_settings)
                .await
                .with_context(|| {
                    format!("failed to fetch provider under OCI reference `{provider_ref}`")
                })
        }
    }
}
