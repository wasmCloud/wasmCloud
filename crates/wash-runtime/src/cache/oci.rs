use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::Result;
use hmac::{Hmac, Mac};
use oci_client::secrets::RegistryAuth;
use sha2::Sha256;

use super::{Cache, DefaultCache};

use crate::oci::{
    AuthId, OciPullPolicy,
    client::TrustId,
    disk_cache::{CacheManager, Principal},
};

const PULL_ENTRY_WEIGHT: u32 = 16 * 1024;

/// OCI retention and maintenance, extending the common cache contract.
///
/// Methods default to no retention. All storage operations are asynchronous and
/// fallible, and returned futures are `Send`. Preserve the supplied key's
/// authorization, transport and trust scope when storing or returning entries.
///
/// Artifact errors fail the pull or push. Completed-pull and credential errors
/// are logged and the operation continues without reuse.
#[async_trait::async_trait]
pub trait OciCache: Cache {
    /// Read bytes, digest and the request's credential-match status together.
    async fn oci_get_artifact(&self, _key: &ArtifactKey) -> Result<Option<ArtifactEntry>> {
        Ok(None)
    }
    /// Publish an artifact after the registry accepted this request's credentials.
    async fn oci_put_artifact(&self, _key: &ArtifactKey, _value: &Pulled) -> Result<()> {
        Ok(())
    }
    /// Record registry acceptance of the request's credentials for this entry.
    async fn oci_record_credentials(&self, _key: &ArtifactKey) -> Result<()> {
        Ok(())
    }
    /// Completed pulls may be retained for at most five seconds. `Always`
    /// bypasses these operations; it can still join an in-flight pull.
    async fn oci_get_pull(&self, _key: &PullKey) -> Result<Option<Arc<Pulled>>> {
        Ok(None)
    }
    async fn oci_put_pull(&self, _key: &PullKey, _value: Arc<Pulled>) -> Result<()> {
        Ok(())
    }
    /// Helper answers, including anonymous access, expire after at most thirty
    /// seconds. Implementations must protect credentials as secrets.
    async fn oci_get_credentials(&self, _key: &CredentialKey) -> Result<Option<RegistryAuth>> {
        Ok(None)
    }
    async fn oci_put_credentials(&self, _key: &CredentialKey, _value: RegistryAuth) -> Result<()> {
        Ok(())
    }
    /// Expire artifacts older than `age`. Called outside the control loop.
    async fn oci_maintain(&self, _age: Duration) -> Result<()> {
        Ok(())
    }
    /// Whether the host should schedule maintenance. Automatic-expiry backends
    /// can return false. This method must not perform I/O.
    fn oci_needs_maintenance(&self) -> bool {
        false
    }
}

impl std::fmt::Debug for dyn OciCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OciCache")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl OciCache for DefaultCache {
    async fn oci_get_artifact(&self, key: &ArtifactKey) -> Result<Option<ArtifactEntry>> {
        self.oci.oci_get_artifact(key).await
    }
    async fn oci_put_artifact(&self, key: &ArtifactKey, value: &Pulled) -> Result<()> {
        self.oci.oci_put_artifact(key, value).await
    }
    async fn oci_record_credentials(&self, key: &ArtifactKey) -> Result<()> {
        self.oci.oci_record_credentials(key).await
    }
    async fn oci_get_pull(&self, key: &PullKey) -> Result<Option<Arc<Pulled>>> {
        Ok(self.oci.pulls.get(key))
    }
    async fn oci_put_pull(&self, key: &PullKey, value: Arc<Pulled>) -> Result<()> {
        self.oci.pulls.insert(key.clone(), value);
        Ok(())
    }
    async fn oci_get_credentials(&self, key: &CredentialKey) -> Result<Option<RegistryAuth>> {
        Ok(self.oci.credentials.get(key))
    }
    async fn oci_put_credentials(&self, key: &CredentialKey, value: RegistryAuth) -> Result<()> {
        self.oci.credentials.insert(key.clone(), value);
        Ok(())
    }
    async fn oci_maintain(&self, age: Duration) -> Result<()> {
        if let Some(directory) = &self.oci.directory {
            crate::oci::disk_cache::expire_artifacts(directory, age).await?;
        }
        Ok(())
    }
    fn oci_needs_maintenance(&self) -> bool {
        self.oci.directory.is_some()
    }
}

/// Validated component bytes and the registry's manifest digest.
#[derive(Clone)]
pub struct Pulled {
    pub bytes: bytes::Bytes,
    pub digest: String,
}

/// An artifact and whether the request's exact credentials last authenticated it.
/// `credentials_match` must only be true after a successful publication or
/// `oci_record_credentials` for the same credential proof and storage scope.
#[derive(Clone)]
pub struct ArtifactEntry {
    pub artifact: Pulled,
    pub credentials_match: bool,
}

/// Reference, principal, credentials, transport, and trust of an artifact request.
/// Debug output deliberately omits credential-derived material.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ArtifactKey {
    pub(crate) reference: String,
    pub(crate) principal: Principal,
    pub(crate) credential: AuthId,
    pub(crate) insecure: bool,
    pub(crate) trust: TrustId,
}

impl ArtifactKey {
    pub(crate) fn new(
        reference: &str,
        auth: &RegistryAuth,
        insecure: bool,
        trust: TrustId,
    ) -> Self {
        Self {
            reference: reference.into(),
            principal: Principal::of(auth),
            credential: AuthId::of(auth),
            insecure,
            trust,
        }
    }
    pub fn reference(&self) -> &str {
        &self.reference
    }

    /// Stable storage identity excluding the password, allowing rotation to
    /// revalidate existing bytes. Use a backend-owned persistent secret; never
    /// publish an unkeyed credential fingerprint. Different secrets isolate stores.
    pub fn storage_key(&self, secret: &[u8]) -> Result<[u8; 32]> {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret)
            .map_err(|error| anyhow::anyhow!("invalid cache secret: {error}"))?;
        mac.update(b"wash-oci-cache-v3\0");
        mac.update(&self.principal.0);
        mac.update(&[u8::from(self.insecure)]);
        mac.update(&self.trust.0);
        mac.update(self.reference.as_bytes());
        Ok(mac.finalize().into_bytes().into())
    }
    /// Proof of the exact credentials, for offline reuse. Store this only after
    /// the runtime calls `oci_put_artifact` or `oci_record_credentials`.
    pub fn credential_proof(&self, secret: &[u8]) -> Result<[u8; 32]> {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret)
            .map_err(|error| anyhow::anyhow!("invalid cache secret: {error}"))?;
        mac.update(b"wash-oci-cache-credential-v1\0");
        mac.update(&self.credential.0);
        Ok(mac.finalize().into_bytes().into())
    }
}

impl std::fmt::Debug for ArtifactKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtifactKey")
            .field("reference", &self.reference)
            .finish_non_exhaustive()
    }
}

/// Full context of a completed pull. Equality includes authorization, trust,
/// policy and timeout. Retain only successful results, for at most five seconds.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PullKey {
    pub(crate) artifact: ArtifactKey,
    pub(crate) pull_policy: OciPullPolicy,
    pub(crate) timeout: Option<Duration>,
}
impl PullKey {
    pub fn artifact(&self) -> &ArtifactKey {
        &self.artifact
    }
    pub fn policy(&self) -> &OciPullPolicy {
        &self.pull_policy
    }
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }
}
impl std::fmt::Debug for PullKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PullKey")
            .field("artifact", &self.artifact)
            .field("policy", &self.pull_policy)
            .finish_non_exhaustive()
    }
}

/// A helper lookup scoped to the registry and Docker configuration location.
/// Anonymous answers are values, distinct from cache misses.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CredentialKey {
    pub registry: String,
    pub config_path: Option<PathBuf>,
}

#[derive(Clone)]
pub(super) struct Storage {
    pub directory: Option<PathBuf>,
    pub pulls: moka::sync::Cache<PullKey, Arc<Pulled>>,
    pub credentials: moka::sync::Cache<CredentialKey, RegistryAuth>,
}
impl Default for Storage {
    fn default() -> Self {
        Self {
            directory: None,
            pulls: pull_cache(256 * 1024 * 1024),
            credentials: moka::sync::Cache::builder()
                .max_capacity(64)
                .time_to_live(Duration::from_secs(30))
                .build(),
        }
    }
}
fn pull_cache(capacity: u64) -> moka::sync::Cache<PullKey, Arc<Pulled>> {
    moka::sync::Cache::builder()
        .max_capacity(capacity)
        .time_to_live(Duration::from_secs(5))
        .weigher(|_: &PullKey, value: &Arc<Pulled>| {
            u32::try_from(value.bytes.len())
                .unwrap_or(u32::MAX)
                .saturating_add(PULL_ENTRY_WEIGHT)
        })
        .build()
}

impl Storage {
    async fn manager(&self, key: &ArtifactKey) -> Result<Option<CacheManager>> {
        match &self.directory {
            Some(dir) => Ok(Some(CacheManager::open_scoped(dir.clone(), key).await?)),
            None => Ok(None),
        }
    }
    pub async fn oci_get_artifact(&self, key: &ArtifactKey) -> Result<Option<ArtifactEntry>> {
        let Some(manager) = self.manager(key).await? else {
            return Ok(None);
        };
        let Some((bytes, digest)) = manager.read_optional(key.reference()).await? else {
            return Ok(None);
        };
        Ok(Some(ArtifactEntry {
            artifact: Pulled {
                bytes: bytes.into(),
                digest,
            },
            credentials_match: manager.credential_matches(key.reference()).await,
        }))
    }
    pub async fn oci_put_artifact(&self, key: &ArtifactKey, value: &Pulled) -> Result<()> {
        if let Some(manager) = self.manager(key).await? {
            manager
                .write_to_cache(key.reference(), &value.bytes, &value.digest)
                .await?;
        }
        Ok(())
    }
    pub async fn oci_record_credentials(&self, key: &ArtifactKey) -> Result<()> {
        if let Some(manager) = self.manager(key).await? {
            manager.record_credential(key.reference()).await?;
        }
        Ok(())
    }
}

/// Standalone OCI calls historically share completed results. Keep bounded
/// default instances per directory; explicitly injected caches never use this.
pub(crate) fn standalone(directory: Option<PathBuf>) -> Arc<dyn OciCache> {
    static DEFAULTS: OnceLock<moka::sync::Cache<Option<PathBuf>, Arc<dyn OciCache>>> =
        OnceLock::new();
    DEFAULTS
        .get_or_init(|| {
            moka::sync::Cache::builder()
                .max_capacity(64)
                .time_to_idle(Duration::from_secs(600))
                .build()
        })
        .get_with(directory.clone(), || {
            let cache = DefaultCache::default();
            Arc::new(match directory {
                Some(dir) => cache.with_oci_directory(dir),
                None => cache,
            })
        })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn key(reference: &str) -> PullKey {
        PullKey {
            artifact: ArtifactKey::new(
                reference,
                &RegistryAuth::Anonymous,
                false,
                TrustId([0; 32]),
            ),
            pull_policy: OciPullPolicy::IfNotPresent,
            timeout: None,
        }
    }

    #[tokio::test]
    async fn completed_pulls_are_bounded_by_bytes_and_entry_overhead() -> Result<()> {
        let capacity = 2 * u64::from(PULL_ENTRY_WEIGHT) + 15;
        let cache = DefaultCache {
            oci: Storage {
                pulls: pull_cache(capacity),
                ..Default::default()
            },
            ..Default::default()
        };
        for reference in ["a", "b", "c"] {
            cache
                .oci_put_pull(
                    &key(reference),
                    Arc::new(Pulled {
                        bytes: bytes::Bytes::from_static(b"12345678"),
                        digest: "sha256:test".into(),
                    }),
                )
                .await?;
        }
        cache.oci.pulls.run_pending_tasks();
        assert!(cache.oci.pulls.weighted_size() <= capacity);
        assert!(cache.oci.pulls.entry_count() < 3);
        Ok(())
    }

    #[tokio::test]
    async fn an_oversized_pull_is_not_retained() -> Result<()> {
        let cache = DefaultCache {
            oci: Storage {
                pulls: pull_cache(u64::from(PULL_ENTRY_WEIGHT) - 1),
                ..Default::default()
            },
            ..Default::default()
        };
        let key = key("oversized");
        cache
            .oci_put_pull(
                &key,
                Arc::new(Pulled {
                    bytes: bytes::Bytes::new(),
                    digest: "sha256:test".into(),
                }),
            )
            .await?;
        cache.oci.pulls.run_pending_tasks();
        assert!(cache.oci_get_pull(&key).await?.is_none());
        Ok(())
    }
}
