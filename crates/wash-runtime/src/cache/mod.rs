//! Replaceable retention for compiled components and OCI artifacts.
//!
//! Supply an `Arc<CacheBackend>` to an engine or host builder. Methods default to
//! no retention, so implementations can support only the domains they need.
//! The runtime owns compilation, authorization, and in-flight coordination.
//! Backends must publish whole entries atomically, preserve key isolation, and
//! never block an async worker. Cache implementations are trusted embedder code.
//!
//! A cache saves work, and a failing one costs only that work. An error from
//! compiled-component, completed-pull or credential retention is logged, then
//! treated as a miss on a read and as nothing kept on a write: the start or
//! pull goes on. OCI artifact errors do propagate, as the disk cache's always
//! have: an entry that cannot be read may be the only copy `Never` may use,
//! and one that cannot be written would silently stop offline reuse.

#[cfg(feature = "oci")]
use std::sync::Arc;
use std::{path::PathBuf, time::Duration};

use anyhow::Result;

pub use crate::engine::{CacheKey, CacheValue};

pub(crate) mod coalescer;
#[cfg(feature = "oci")]
pub(crate) mod oci;
#[cfg(feature = "oci")]
pub use oci::{ArtifactEntry, ArtifactKey, CredentialKey, OciCache, PullKey, Pulled};
#[cfg(feature = "oci")]
pub use oci_client::secrets::RegistryAuth;

/// The backend accepted by engine and host builders. With OCI enabled, it
/// implements [`OciCache`] as well as [`Cache`]; an empty `impl OciCache` opts
/// out of OCI retention. Without OCI, only [`Cache`] is required.
#[cfg(feature = "oci")]
pub type CacheBackend = dyn OciCache;
/// The backend accepted by engine and host builders without OCI support.
#[cfg(not(feature = "oci"))]
pub type CacheBackend = dyn Cache;

/// Storage operations used by the runtime. All futures are `Send`.
///
/// A backend may decline to retain a value. A successful write does not promise
/// a subsequent hit, but a returned entry must belong to the exact supplied key.
/// Compiled values are process-local and engine-specific: do not deserialize
/// arbitrary machine code or share a value between different [`CacheKey`]s.
/// Decorators can delegate individual methods to [`DefaultCache`].
#[async_trait::async_trait]
pub trait Cache: Send + Sync + 'static {
    /// A diagnostic label for this implementation or instance. Must not perform
    /// I/O or expose secrets. Names need not be unique and do not affect isolation.
    /// Override to return an instance-specific name stored by the backend.
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// Look up a compiled component for the exact engine/digest key.
    async fn get_compiled(&self, _key: &CacheKey) -> Result<Option<CacheValue>> {
        Ok(None)
    }
    /// Retain the component and its metadata as one value.
    async fn put_compiled(&self, _key: &CacheKey, _value: CacheValue) -> Result<()> {
        Ok(())
    }
    /// Paths guests must not mount. Called during construction, without I/O.
    fn reserved_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

impl std::fmt::Debug for dyn Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

/// The built-in backend: bounded memory retention and optional OCI disk storage.
/// Clones share memory entries. No directory is created until an artifact is used.
#[derive(Clone)]
pub struct DefaultCache {
    pub(crate) compiled: moka::sync::Cache<CacheKey, CacheValue>,
    #[cfg(feature = "oci")]
    oci: oci::Storage,
}

impl Default for DefaultCache {
    fn default() -> Self {
        Self::new(100, Duration::from_secs(600))
    }
}

impl DefaultCache {
    /// Configure compiled-entry capacity and time to idle.
    pub fn new(capacity: u64, idle: Duration) -> Self {
        Self {
            compiled: moka::sync::Cache::builder()
                .max_capacity(capacity)
                .time_to_idle(idle)
                .build(),
            #[cfg(feature = "oci")]
            oci: oci::Storage::default(),
        }
    }

    /// Enable the existing credential-scoped disk cache at `directory`.
    #[cfg(feature = "oci")]
    pub fn with_oci_directory(mut self, directory: PathBuf) -> Self {
        self.oci = oci::Storage {
            directory: Some(directory),
            ..Default::default()
        };
        self
    }
}

#[async_trait::async_trait]
impl Cache for DefaultCache {
    async fn get_compiled(&self, key: &CacheKey) -> Result<Option<CacheValue>> {
        Ok(self.compiled.get(key))
    }
    async fn put_compiled(&self, key: &CacheKey, value: CacheValue) -> Result<()> {
        self.compiled.insert(key.clone(), value);
        Ok(())
    }
    fn reserved_paths(&self) -> Vec<PathBuf> {
        #[cfg(feature = "oci")]
        {
            self.oci.directory.iter().cloned().collect()
        }
        #[cfg(not(feature = "oci"))]
        {
            Vec::new()
        }
    }
}

/// An identity held by the key keeps the backend alive and prevents pointer
/// reuse from confusing requests after an embedder drops its last handle.
#[cfg(feature = "oci")]
#[derive(Clone)]
pub(crate) struct Identity(pub(crate) Arc<dyn OciCache>);
#[cfg(feature = "oci")]
impl PartialEq for Identity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
#[cfg(feature = "oci")]
impl Eq for Identity {}
#[cfg(feature = "oci")]
impl std::hash::Hash for Identity {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0).cast::<()>(), state);
    }
}
