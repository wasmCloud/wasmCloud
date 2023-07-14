// Adapted from
// https://github.com/wasmCloud/wasmcloud-otp/blob/5f13500646d9e077afa1fca67a3fe9c8df5f3381/host_core/native/hostcore_wasmcloud_native/src/par.rs

use std::env::consts::{ARCH, OS};
use std::env::temp_dir;
use std::path::{Path, PathBuf};
use std::str;

use anyhow::{anyhow, Context};
use provider_archive::ProviderArchive;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;
use wascap::jwt;

fn normalize_for_filename(input: &str) -> String {
    input
        .to_lowercase()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
}

pub(super) async fn create(path: impl AsRef<Path>) -> anyhow::Result<Option<File>> {
    let path = path.as_ref();
    // Check if the file exists and return
    if fs::metadata(path).await.is_ok() {
        return Ok(None);
    }
    let dir = path.parent().context("failed to determine parent path")?;
    fs::create_dir_all(dir)
        .await
        .context("failed to create parent directory")?;

    let mut open_opts = OpenOptions::new();
    open_opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    open_opts.mode(0o755);
    open_opts
        .open(path)
        .await
        .map(Some)
        .context("failed to open path")
}

fn native_target() -> String {
    format!("{ARCH}-{OS}")
}

// TODO: this should respect a host ID in the future so that cached data
// from two different hosts can't cross over the veil
pub fn cache_path(
    claims: &jwt::Claims<jwt::CapabilityProvider>,
    link_name: impl AsRef<str>,
) -> PathBuf {
    let metadata = claims.metadata.as_ref();
    #[allow(clippy::cast_possible_truncation)] // Legacy implementation casts here
    let revision = metadata
        .and_then(|jwt::CapabilityProvider { rev, .. }| *rev)
        .filter(|rev| *rev > 0)
        .unwrap_or(claims.issued_at as _);
    let contract = normalize_for_filename(
        metadata
            .map(|jwt::CapabilityProvider { capid, .. }| capid.as_str())
            .unwrap_or_default(),
    );
    let link_name = normalize_for_filename(link_name.as_ref());

    let mut cache = temp_dir();
    cache.push("wasmcloudcache");
    cache.push(&claims.subject);
    cache.push(revision.to_string());
    cache.push(format!("{contract}_{link_name}"));
    #[cfg(windows)]
    cache.set_extension("exe");
    cache
}

pub async fn read(
    path: impl AsRef<Path>,
    link_name: impl AsRef<str>,
) -> anyhow::Result<(PathBuf, jwt::Claims<jwt::CapabilityProvider>)> {
    let par = ProviderArchive::try_load_target_from_file(path, &native_target())
        .await
        .map_err(|e| anyhow!(e).context("failed to load provider archive"))?;
    let claims = par.claims().context("claims missing")?;

    let exe = cache_path(&claims, link_name);
    // Only write the file if it doesn't exist
    if let Some(mut file) = create(&exe).await? {
        let target = native_target();
        let buf = par
            .target_bytes(&target)
            .with_context(|| format!("target `{target}` not found"))?;
        file.write_all(&buf).await.context("failed to write")?;
        file.flush().await.context("failed to flush")?;
    }
    Ok((exe, claims))
}
