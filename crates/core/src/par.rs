use std::env::consts::{ARCH, OS};
use std::env::temp_dir;
use std::path::{Path, PathBuf};
use std::str;

use anyhow::{anyhow, Context, Result};
use provider_archive::ProviderArchive;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;
use wascap::jwt;

fn normalize_for_filename(input: &str) -> String {
    input
        .to_lowercase()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
}

/// Whether to use the par file cache
#[derive(Default, Clone, PartialEq, Eq)]
pub enum UseParFileCache {
    /// Use the par file cache
    Ignore,
    /// Use the par file cache
    #[default]
    Use,
}

fn native_target() -> String {
    format!("{ARCH}-{OS}")
}

/// Returns the path to the cache file for a provider
///
/// # Arguments
/// * `host_id` - The host ID this provider is starting on. Required in order to isolate provider caches
///   for different hosts
/// * `provider_ref` - The provider reference, e.g. file or OCI
pub fn cache_path(host_id: impl AsRef<str>, provider_ref: impl AsRef<str>) -> PathBuf {
    let provider_ref = normalize_for_filename(provider_ref.as_ref());

    let mut cache = temp_dir();
    cache.push("wasmcloudcache");
    cache.push(host_id.as_ref());
    cache.push(&provider_ref);
    #[cfg(windows)]
    cache.set_extension("exe");
    cache
}

pub(super) async fn create(path: impl AsRef<Path>) -> Result<Option<File>> {
    let path = path.as_ref();
    // Check if the file exists and return
    if fs::metadata(path).await.is_ok() {
        return Ok(None);
    }
    let dir = path.parent().context("failed to determine parent path")?;
    fs::create_dir_all(dir)
        .await
        .context("failed to create parent directory")?;

    open_file(path).await
}

async fn open_file(path: impl AsRef<Path>) -> Result<Option<File>> {
    let path = path.as_ref();
    let mut open_opts = OpenOptions::new();
    open_opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    open_opts.mode(0o755);
    open_opts
        .open(path)
        .await
        .map(Some)
        .with_context(|| format!("failed to open path [{}]", path.display()))
}

/// Reads a provider archive from the given path and writes it to the cache
///
/// # Arguments
/// * `path` - The path to the provider archive
/// * `host_id` - The host ID this provider is starting on. Required in order to isolate provider caches
///   for different hosts
/// * `provider_ref` - The reference to the provider (e.g. file or OCI). Required to cache provider for future fetches
pub async fn read(
    path: impl AsRef<Path>,
    host_id: impl AsRef<str>,
    provider_ref: impl AsRef<str>,
    cache: UseParFileCache,
) -> Result<(PathBuf, Option<jwt::Token<jwt::CapabilityProvider>>)> {
    let par = ProviderArchive::try_load_target_from_file(path, &native_target())
        .await
        .map_err(|e| anyhow!(e).context("failed to load provider archive"))?;
    let claims = par.claims_token();
    let exe = cache_path(host_id, provider_ref);

    let new_file = create(&exe).await?;
    let mut file = match (cache, new_file) {
        (UseParFileCache::Use, None) => {
            return Ok((exe, claims));
        }
        (UseParFileCache::Ignore, None) => open_file(&exe)
            .await?
            .with_context(|| format!("failed to open file [{}]", exe.display()))?,
        (UseParFileCache::Use, Some(file)) | (UseParFileCache::Ignore, Some(file)) => file,
    };

    let target = native_target();
    let buf = par
        .target_bytes(&target)
        .with_context(|| format!("target `{target}` not found"))?;
    file.write_all(&buf).await.context("failed to write")?;
    file.flush().await.context("failed to flush")?;

    Ok((exe, claims))
}
