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

pub use local::{Host as LocalHost, HostConfig as LocalHostConfig};
pub use wasmbus::{Host as WasmbusHost, HostConfig as WasmbusHostConfig};

pub use url;

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context as _};
use tokio::fs;
use tracing::instrument;
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
                    // TODO: Support other schemes
                    scheme => bail!("unsupported scheme `{scheme}`"),
                }
            }
            Err(url::ParseError::RelativeUrlWithoutBase) => Ok(Self::Oci(s)), // TODO: Validate
            Err(e) => {
                bail!(anyhow!(e).context(format!("failed to parse reference `{s}`")))
            }
        }
    }
}

/// Fetch an actor from a reference.
#[instrument(skip(actor_ref))]
pub async fn fetch_actor(actor_ref: impl AsRef<str>) -> anyhow::Result<Vec<u8>> {
    match ResourceRef::try_from(actor_ref.as_ref())? {
        ResourceRef::File(actor_ref) => fs::read(actor_ref).await.context("failed to read actor"),
        ResourceRef::Bindle(actor_ref) => crate::bindle::fetch_actor(None, &actor_ref)
            .await
            .with_context(|| format!("failed to fetch actor under Bindle reference `{actor_ref}`")),
        // TODO: Set config
        ResourceRef::Oci(actor_ref) => crate::oci::fetch_actor(None, &actor_ref, true, vec![])
            .await
            .with_context(|| format!("failed to fetch actor under OCI reference `{actor_ref}`")),
    }
}

/// Fetch a provider from a reference.
#[instrument(skip(provider_ref, link_name))]
pub async fn fetch_provider(
    provider_ref: impl AsRef<str>,
    link_name: impl AsRef<str>,
) -> anyhow::Result<(PathBuf, jwt::Claims<jwt::CapabilityProvider>)> {
    match ResourceRef::try_from(provider_ref.as_ref())? {
        ResourceRef::File(provider_ref) => par::read(provider_ref, link_name)
            .await
            .context("failed to read provider"),
        ResourceRef::Bindle(provider_ref) => {
            crate::bindle::fetch_provider(None, &provider_ref, link_name)
                .await
                .with_context(|| {
                    format!("failed to fetch provider under Bindle reference `{provider_ref}`")
                })
        }
        // TODO: Set config
        ResourceRef::Oci(provider_ref) => {
            crate::oci::fetch_provider(None, &provider_ref, true, vec![], link_name)
                .await
                .with_context(|| {
                    format!("failed to fetch provider under OCI reference `{provider_ref}`")
                })
        }
    }
}
