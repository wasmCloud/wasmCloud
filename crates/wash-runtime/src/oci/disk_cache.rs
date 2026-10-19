use super::AuthId;
use anyhow::{Context, Result, bail};
use oci_client::secrets::RegistryAuth;
use sha2::{Digest, Sha256};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};
use tracing::debug;

/// The random secret entry names are keyed with, in the cache root.
const KEY_FILE: &str = "cache.key";
/// Prefix of every entry this version reads and writes.
const ENTRY_PREFIX: &str = "v2-";

/// Cache manager for OCI artifacts
pub(super) struct CacheManager {
    cache_dir: PathBuf,
    auth: AuthId,
    key: [u8; 32],
}

impl CacheManager {
    /// Offline reuse is confined to the credentials that populated the entry.
    /// Legacy entries without an authentication context are intentionally not
    /// reused: their contents cannot establish who was authorized to fetch them.
    ///
    /// Creates the cache root's key on first use.
    pub(super) async fn open(cache_dir: PathBuf, auth: &RegistryAuth) -> Result<Self> {
        let key = tokio::task::spawn_blocking({
            let cache_dir = cache_dir.clone();
            move || load_or_create_key(&cache_dir)
        })
        .await
        .context("OCI cache key task failed")??;
        Ok(Self {
            cache_dir,
            auth: AuthId::of(auth),
            key,
        })
    }

    /// Get the cache directory for a given OCI reference
    ///
    /// Named by an HMAC keyed with the cache root's secret: the name derives
    /// from the credentials, and an unkeyed hash of them would let anyone who
    /// can list the directory test password guesses offline.
    pub(super) fn get_cache_dir(&self, reference: &str) -> PathBuf {
        let hash = hmac_sha256(
            &self.key,
            &[
                b"wash-oci-cache-v2\0".as_slice(),
                self.auth.0.as_slice(),
                reference.as_bytes(),
            ],
        );
        self.cache_dir.join(format!("{ENTRY_PREFIX}{hash:x}"))
    }

    /// Get the cache path for the component .wasm data
    pub(super) fn get_component_path(&self, reference: &str) -> PathBuf {
        self.get_cache_dir(reference).join("component.wasm")
    }

    /// Get the cache path for the digest file
    pub(super) fn get_digest_path(&self, reference: &str) -> PathBuf {
        let cache_dir = self.get_cache_dir(reference);
        cache_dir.join("digest")
    }

    /// Check if an artifact is cached (both component and digest must exist)
    pub(super) async fn is_cached(&self, reference: &str) -> bool {
        let component_path = self.get_component_path(reference);
        let digest_path = self.get_digest_path(reference);
        tokio::fs::metadata(&component_path).await.is_ok()
            && tokio::fs::metadata(&digest_path).await.is_ok()
    }

    /// Read cached artifact, returning (component_data, digest)
    pub(super) async fn read_cached(&self, reference: &str) -> Result<(Vec<u8>, String)> {
        let component_path = self.get_component_path(reference);
        let digest_path = self.get_digest_path(reference);

        debug!(component_path = %component_path.display(), digest_path = %digest_path.display(), "reading cached artifact");

        let component_data = tokio::fs::read(&component_path).await.with_context(|| {
            format!(
                "failed to read cached component at {}",
                component_path.display()
            )
        })?;

        let digest = tokio::fs::read_to_string(&digest_path)
            .await
            .with_context(|| {
                format!("failed to read cached digest at {}", digest_path.display())
            })?;

        Ok((component_data, digest.trim().to_string()))
    }

    /// Write artifact and digest to cache
    ///
    /// Each file is written beside its final name and renamed into place, so
    /// a concurrent reader or a crash never sees a truncated component. The
    /// digest goes last: an entry is only complete once it exists.
    pub(super) async fn write_to_cache(
        &self,
        reference: &str,
        data: &[u8],
        digest: &str,
    ) -> Result<()> {
        let cache_dir = self.get_cache_dir(reference);

        debug!(cache_dir = %cache_dir.display(), "writing to cache");

        tokio::fs::create_dir_all(&cache_dir)
            .await
            .with_context(|| format!("failed to create cache directory {}", cache_dir.display()))?;

        write_replacing(&cache_dir, "component.wasm", data).await?;
        write_replacing(&cache_dir, "digest", digest.as_bytes()).await?;

        Ok(())
    }
}

/// Expire old artifacts from the cache, and remove entries written before
/// entries were scoped by credentials: they are never read again.
pub(super) async fn expire_artifacts(cache_dir: &Path, age: Duration) -> Result<()> {
    // walk the cache directory, looking for artifact dirs
    // and remove those older than the specified age
    let mut dir_entries = match tokio::fs::read_dir(cache_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("failed to read cache directory"),
    };
    while let Some(entry) = dir_entries
        .next_entry()
        .await
        .context("failed to read cache entry")?
    {
        // Removing the key would orphan every entry keyed with it.
        if entry.file_name() == KEY_FILE {
            continue;
        }
        let metadata = entry
            .metadata()
            .await
            .context("failed to read cache entry metadata")?;
        if metadata.is_dir() && is_legacy_entry(&entry.path(), &entry.file_name()).await {
            debug!(path = %entry.path().display(), "removing unscoped legacy cache entry");
            tokio::fs::remove_dir_all(entry.path())
                .await
                .context("failed to remove legacy cache entry")?;
            continue;
        }
        if let Ok(modified) = metadata.modified() {
            let modified_duration = modified
                .elapsed()
                .context("failed to compute modified duration")?;
            if modified_duration > age {
                debug!(path = %entry.path().display(), "expiring cached artifact");
                if metadata.is_dir() {
                    tokio::fs::remove_dir_all(entry.path())
                        .await
                        .context("failed to remove expired cache entry")?;
                } else {
                    // A key file left half-published by a crash.
                    tokio::fs::remove_file(entry.path())
                        .await
                        .context("failed to remove expired cache file")?;
                }
            }
        }
    }

    Ok(())
}

/// An entry in the layout before entries were scoped by credentials:
/// `<sanitized reference>_<hash8>/` holding a `digest`. Matched narrowly, since
/// the cache root may be a directory the operator also uses for other things.
async fn is_legacy_entry(path: &Path, name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    !name.starts_with(ENTRY_PREFIX)
        && name
            .rsplit_once('_')
            .is_some_and(|(_, hash)| hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        && tokio::fs::metadata(path.join("digest")).await.is_ok()
}

/// Replace `dir/name` with `data` without a reader ever seeing it partly
/// written.
async fn write_replacing(dir: &Path, name: &str, data: &[u8]) -> Result<()> {
    let path = dir.join(name);
    let temp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    tokio::fs::write(&temp, data)
        .await
        .with_context(|| format!("failed to write cache file {}", temp.display()))?;
    if let Err(err) = tokio::fs::rename(&temp, &path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err).with_context(|| format!("failed to write cache file {}", path.display()));
    }
    Ok(())
}

/// The cache root's secret, created on first use and readable only by its
/// owner on Unix. It is never rotated: entries keyed with an earlier secret
/// would simply be fetched again.
fn load_or_create_key(cache_dir: &Path) -> Result<[u8; 32]> {
    let path = cache_dir.join(KEY_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => return parse_key(&path, &bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", path.display()));
        }
    }
    std::fs::create_dir_all(cache_dir)
        .with_context(|| format!("failed to create cache directory {}", cache_dir.display()))?;

    // Two v4 UUIDs: 244 bits from the OS random source.
    let mut key = Vec::with_capacity(32);
    key.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    key.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    let temp = cache_dir.join(format!("{KEY_FILE}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let written = options.open(&temp).and_then(|mut file| {
        file.write_all(&key)?;
        file.sync_all()
    });
    if let Err(err) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(err).with_context(|| format!("failed to write {}", temp.display()));
    }
    // Published whole, and only if no other process got there first, so every
    // process sharing the root agrees on one key.
    let published = match std::fs::hard_link(&temp, &path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        // No hard links on this filesystem. A racing process may replace the
        // key, which only costs the entries it keyed.
        Err(_) => std::fs::rename(&temp, &path),
    };
    let _ = std::fs::remove_file(&temp);
    published.with_context(|| format!("failed to create {}", path.display()))?;
    let bytes =
        std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_key(&path, &bytes)
}

fn parse_key(path: &Path, bytes: &[u8]) -> Result<[u8; 32]> {
    match <[u8; 32]>::try_from(bytes) {
        Ok(key) => Ok(key),
        Err(_) => bail!(
            "OCI cache key {} is not 32 bytes; remove it to start a new cache",
            path.display()
        ),
    }
}

/// HMAC-SHA256 (RFC 2104) of `parts`, concatenated, under a 32-byte key.
fn hmac_sha256(key: &[u8; 32], parts: &[&[u8]]) -> sha2::digest::Output<Sha256> {
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for ((inner, outer), byte) in inner_pad.iter_mut().zip(outer_pad.iter_mut()).zip(key) {
        *inner ^= byte;
        *outer ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    for part in parts {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    outer.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 test case 1's message under a 32-byte key, as
    /// `openssl dgst -sha256 -mac HMAC -macopt hexkey:0b...` computes it.
    #[test]
    fn hmac_matches_a_known_answer() {
        let key = [0x0bu8; 32];
        let mac = hmac_sha256(&key, &[b"Hi ".as_slice(), b"There".as_slice()]);
        assert_eq!(format!("{mac:x}"), KNOWN_MAC);
    }
    const KNOWN_MAC: &str = "198a607eb44bfbc69903a0f1cf2bbdc5ba0aa3f3d9ae3c1c7a3b1696a0b68cf7";

    #[tokio::test]
    async fn the_key_is_created_once_and_reused() {
        let dir = tempfile::TempDir::new().unwrap();
        let auth = RegistryAuth::Anonymous;
        let first = CacheManager::open(dir.path().to_path_buf(), &auth)
            .await
            .unwrap();
        let second = CacheManager::open(dir.path().to_path_buf(), &auth)
            .await
            .unwrap();
        assert_eq!(first.get_cache_dir("r"), second.get_cache_dir("r"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "key file mode {mode:o}");
        }

        // Another cache root has its own key, so the same reference and
        // credentials name a different entry.
        let other = tempfile::TempDir::new().unwrap();
        let other = CacheManager::open(other.path().to_path_buf(), &auth)
            .await
            .unwrap();
        assert_ne!(
            first.get_cache_dir("r").file_name(),
            other.get_cache_dir("r").file_name()
        );
    }

    #[tokio::test]
    async fn cleanup_removes_legacy_entries_and_keeps_the_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = CacheManager::open(dir.path().to_path_buf(), &RegistryAuth::Anonymous)
            .await
            .unwrap();
        manager
            .write_to_cache("registry/ns/c:1", b"bytes", "sha256:aa")
            .await
            .unwrap();
        let legacy = dir.path().join("registry_ns_c_1_0123abcd");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("digest"), "sha256:aa").unwrap();
        let unrelated = dir.path().join("not_a_cache_entry");
        std::fs::create_dir_all(&unrelated).unwrap();

        expire_artifacts(dir.path(), Duration::from_secs(3600))
            .await
            .unwrap();
        assert!(!legacy.exists());
        assert!(unrelated.exists());
        assert!(dir.path().join(KEY_FILE).exists());
        assert!(manager.is_cached("registry/ns/c:1").await);
    }
}
