//! Reusable code for downloading tarballs from GitHub releases

use anyhow::{anyhow, bail, Result};
use async_compression::tokio::bufread::GzipDecoder;
#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};
use std::{ffi::OsStr, io::Cursor};
use tokio::fs::{create_dir_all, metadata, File};
use tokio_stream::StreamExt;
use tokio_tar::Archive;
use wasmcloud_core::tls::NativeRootsExt;

pub const DOWNLOAD_CLIENT_USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

pub const GITHUB_WASMCLOUD_ORG: &str = "wasmCloud";
pub const GITHUB_WASMCLOUD_WASMCLOUD_REPO: &str = "wasmCloud";
pub const GITHUB_WASMCLOUD_WADM_REPO: &str = "wadm";

mod api;
pub use api::*;

/// Reusable function to download a release tarball from GitHub and extract an embedded binary to a specified directory
///
/// # Arguments
///
/// * `url` - URL of the GitHub release artifact tarball (Usually in the form of <https://github.com>/<owner>/<repo>/releases/download/<tag>/<artifact>.tar.gz)
/// * `dir` - Directory on disk to install the binary into. This will be created if it doesn't exist
/// * `bin_name` - Name of the binary inside of the tarball, e.g. `nats-server` or `wadm`
/// # Examples
///
/// ```rust,ignore
/// # #[tokio::main]
/// # async fn main() {
/// let url = "https://github.com/wasmCloud/wadm/releases/download/v0.4.0-alpha.1/wadm-v0.4.0-alpha.1-linux-amd64.tar.gz";
/// let res = download_binary_from_github(url, "/tmp/", "wadm").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wadm");
/// # }
/// ```
pub async fn download_binary_from_github<P>(url: &str, dir: P, bin_name: &str) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let bin_path = dir.as_ref().join(bin_name);
    // Download release tarball
    let body = match get_download_client()?.get(url).send().await {
        Ok(resp) => resp.bytes().await?,
        Err(e) => bail!("Failed to request release tarball: {:?}", e),
    };
    let cursor = Cursor::new(body);
    let mut bin_tarball = Archive::new(Box::new(GzipDecoder::new(cursor)));

    // Look for binary within tarball and only extract that
    let mut entries = bin_tarball.entries()?;
    while let Some(res) = entries.next().await {
        let mut entry = res.map_err(|e| {
            anyhow!(
                "Failed to retrieve file from archive, ensure {bin_name} exists. Original error: {e}",
            )
        })?;
        if let Ok(tar_path) = entry.path() {
            match tar_path.file_name() {
                Some(name) if name == OsStr::new(bin_name) => {
                    // Ensure target directory exists
                    create_dir_all(&dir).await?;
                    let mut bin_file = File::create(&bin_path).await?;
                    // Make binary executable
                    #[cfg(target_family = "unix")]
                    {
                        let mut permissions = bin_file.metadata().await?.permissions();
                        // Read/write/execute for owner and read/execute for others. This is what `cargo install` does
                        permissions.set_mode(0o755);
                        bin_file.set_permissions(permissions).await?;
                    }

                    tokio::io::copy(&mut entry, &mut bin_file).await?;
                    return Ok(bin_path);
                }
                // Ignore all other files in the tarball
                _ => (),
            }
        }
    }

    bail!("{bin_name} binary could not be installed, please see logs")
}

/// Helper function to determine if the provided binary is present in a directory
#[allow(unused)]
pub async fn is_bin_installed<P>(dir: P, bin_name: &str) -> bool
where
    P: AsRef<Path>,
{
    metadata(dir.as_ref().join(bin_name))
        .await
        .is_ok_and(|m| m.is_file())
}

/// Helper function to set up a reqwest client for performing the download
pub fn get_download_client() -> Result<reqwest::Client> {
    get_download_client_with_user_agent(DOWNLOAD_CLIENT_USER_AGENT)
}

/// Helper function to set up a reqwest client for performing the download with a user agent
pub fn get_download_client_with_user_agent(user_agent: &str) -> Result<reqwest::Client> {
    let proxy_username = std::env::var("WASH_PROXY_USERNAME").unwrap_or_default();
    let proxy_password = std::env::var("WASH_PROXY_PASSWORD").unwrap_or_default();

    let mut builder = reqwest::ClientBuilder::default()
        .user_agent(user_agent)
        .with_native_certificates();

    if let Ok(http_proxy) = std::env::var("HTTP_PROXY").or_else(|_| std::env::var("http_proxy")) {
        let mut proxy = reqwest::Proxy::http(http_proxy)?.no_proxy(reqwest::NoProxy::from_env());
        if !proxy_username.is_empty() && !proxy_password.is_empty() {
            proxy = proxy.basic_auth(&proxy_username, &proxy_password);
        }
        builder = builder.proxy(proxy);
    }

    if let Ok(https_proxy) = std::env::var("HTTPS_PROXY").or_else(|_| std::env::var("https_proxy"))
    {
        let mut proxy = reqwest::Proxy::https(https_proxy)?.no_proxy(reqwest::NoProxy::from_env());
        if !proxy_username.is_empty() && !proxy_password.is_empty() {
            proxy = proxy.basic_auth(&proxy_username, &proxy_password);
        }
        builder = builder.proxy(proxy);
    }

    Ok(builder.build()?)
}
