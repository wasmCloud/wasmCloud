use super::AuthId;
use anyhow::{Context, Result};
use oci_client::secrets::RegistryAuth;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Duration};
use tracing::debug;

/// Cache manager for OCI artifacts
pub(super) struct CacheManager {
    cache_dir: PathBuf,
    auth: AuthId,
}

impl CacheManager {
    /// Offline reuse is confined to the credentials that populated the entry.
    /// Legacy entries without an authentication context are intentionally not
    /// reused: their contents cannot establish who was authorized to fetch them.
    pub(super) fn new(cache_dir: PathBuf, auth: &RegistryAuth) -> Self {
        Self {
            cache_dir,
            auth: AuthId::of(auth),
        }
    }

    /// Expire old artifacts from the cache
    pub(super) async fn expire_artifacts(&self, age: Duration) -> Result<()> {
        // walk the cache directory, looking for artifact dirs
        // and remove those older than the specified age
        let mut dir_entries = match tokio::fs::read_dir(&self.cache_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).context("failed to read cache directory"),
        };
        while let Some(entry) = dir_entries
            .next_entry()
            .await
            .context("failed to read cache entry")?
        {
            let metadata = entry
                .metadata()
                .await
                .context("failed to read cache entry metadata")?;
            if let Ok(modified) = metadata.modified() {
                let modified_duration = modified
                    .elapsed()
                    .context("failed to compute modified duration")?;
                if modified_duration > age {
                    debug!(path = %entry.path().display(), "expiring cached artifact");
                    tokio::fs::remove_dir_all(entry.path())
                        .await
                        .context("failed to remove expired cache entry")?;
                }
            }
        }

        Ok(())
    }

    /// Get the cache directory for a given OCI reference
    pub(super) fn get_cache_dir(&self, reference: &str) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(b"wash-oci-cache-v2\0");
        hasher.update(self.auth.0);
        hasher.update(reference.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        self.cache_dir.join(format!("v2-{hash}"))
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
    pub(super) async fn write_to_cache(
        &self,
        reference: &str,
        data: &[u8],
        digest: &str,
    ) -> Result<()> {
        let component_path = self.get_component_path(reference);
        let digest_path = self.get_digest_path(reference);

        debug!(component_path = %component_path.display(), digest_path = %digest_path.display(), "writing to cache");

        // Create cache directory
        let cache_dir = self.get_cache_dir(reference);
        tokio::fs::create_dir_all(&cache_dir)
            .await
            .with_context(|| format!("failed to create cache directory {}", cache_dir.display()))?;

        // Write component data
        tokio::fs::write(&component_path, data)
            .await
            .with_context(|| {
                format!(
                    "failed to write component to cache at {}",
                    component_path.display()
                )
            })?;

        // Write digest
        tokio::fs::write(&digest_path, digest)
            .await
            .with_context(|| {
                format!(
                    "failed to write digest to cache at {}",
                    digest_path.display()
                )
            })?;

        Ok(())
    }
}
