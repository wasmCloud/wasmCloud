//! # Multiplexed WASI KeyValue (implements-routed)
//!
//! Binds `wasi:keyvalue` via the component-model `(implements ..)` /
//! `named_imports` mechanism so a single component can import the same
//! interface multiple times and have each import backed by a *different*
//! backend (e.g. one `wasi:keyvalue/store` import backed by redis and another
//! by NATS).
//!
//! Each named import is resolved by a `lookup` closure to a [`KvBackend`]
//! (the wasmtime "implements id"); the embedder-chosen id is threaded into
//! every host method, including the `bucket` resource methods.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use wasmtime::component::Resource;

use crate::engine::ctx::{SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::multiplex::{BackendProvider, Multiplexer};
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

mod bindings {
    wasmtime::component::bindgen!({
        world: "keyvalue",
        imports: { default: async | trappable | tracing },
        named_imports: {
            "wasi:keyvalue/store": super::KvId,
            "wasi:keyvalue/atomics": super::KvId,
            "wasi:keyvalue/batch": super::KvId,
        },
        with: {
            "wasi:keyvalue/store.bucket": super::KvBucket,
        },
    });
}

pub use bindings::wasi::keyvalue::store::{Error as StoreError, KeyResponse};

/// The "implements id" threaded through every keyvalue host method: the backend
/// a given named import is bound to. `Arc` so it is cheaply `Clone`d into each
/// per-import closure, as `named_imports` requires.
pub type KvId = Arc<dyn KvBackend>;

/// A bucket resource. It remembers which backend it was opened through so that
/// every subsequent operation (get/set/atomics/batch) routes to that backend,
/// regardless of which interface the call arrived on.
pub struct KvBucket {
    backend: KvId,
    /// The bucket identifier passed to `store.open`.
    name: String,
}

impl KvBucket {
    pub(crate) fn new(backend: KvId, name: String) -> Self {
        Self { backend, name }
    }

    pub(crate) fn backend(&self) -> &KvId {
        &self.backend
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

/// A value together with its backend-native version token. The token is opaque
/// and only equality is meaningful, but it MUST change on every write to the key
/// (NATS revision, a per-bucket counter, ...). Never a hash of the value, or a
/// value that returns to identical bytes (A → B → A) would reuse a version and
/// defeat the ABA check.
#[derive(Clone, Debug)]
pub struct Versioned {
    pub value: Vec<u8>,
    pub version: String,
}

/// Preconditions for a [`KvBackend::swap`]. At least one must be set (enforced
/// by the host layer). Every set condition must hold for the write to apply.
#[derive(Default, Clone, Debug)]
pub struct CasGuard {
    /// Require the entry's current version to equal this (the ABA-safe check).
    pub require_version: Option<String>,
    /// Require the entry's current value to equal this (ABA-prone).
    pub require_value: Option<Vec<u8>>,
}

/// Outcome of a [`KvBackend::swap`].
#[derive(Clone, Debug)]
pub enum CasOutcome {
    /// Preconditions held; the new value was written.
    Swapped,
    /// A precondition failed; nothing was written. Carries the current entry
    /// (`None` if the key is absent) so the caller can recompute and retry.
    Stale(Option<Versioned>),
}

/// A keyvalue backend (redis, NATS, in-memory, ...). Operations are keyed by
/// the opened bucket `name` plus the per-call key(s); the backend owns its own
/// connection/state. This is the unified surface that the per-interface host
/// trait impls below dispatch onto.
#[async_trait::async_trait]
pub trait KvBackend: Send + Sync {
    /// Validate/create the bucket named `identifier`. Called from `store.open`.
    async fn open(&self, identifier: &str) -> Result<(), StoreError>;
    async fn get(&self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    async fn set(&self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError>;
    async fn delete(&self, bucket: &str, key: &str) -> Result<(), StoreError>;
    async fn exists(&self, bucket: &str, key: &str) -> Result<bool, StoreError>;
    /// List keys, page-by-page via an opaque `u64` cursor. NOTE: paging is
    /// offset/scan-based, so concurrent writes between pages may cause a key to
    /// be skipped or repeated — the WIT permits an out-of-date listing.
    async fn list_keys(&self, bucket: &str, cursor: Option<u64>)
    -> Result<KeyResponse, StoreError>;
    /// Atomically add the signed `delta` to the counter at `key`, returning the
    /// new value. A negative `delta` decrements (the `wasmcloud:keyvalue` counter
    /// is signed, like Redis/DynamoDB/FoundationDB). NOTE: the counter is stored
    /// in a backend-specific encoding (in-memory little-endian i64, NATS
    /// big-endian i64, redis integer, filesystem decimal text), so a counter is
    /// not portable across backends, and a value written via `set` is only a
    /// valid counter if it matches that backend's encoding.
    async fn increment(&self, bucket: &str, key: &str, delta: i64) -> Result<i64, StoreError>;
    #[allow(clippy::type_complexity)]
    async fn get_many(
        &self,
        bucket: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, StoreError>;
    async fn set_many(
        &self,
        bucket: &str,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), StoreError>;
    async fn delete_many(&self, bucket: &str, keys: Vec<String>) -> Result<(), StoreError>;

    /// Set `key` to `value` only if it is absent (the `set` `if-not-exists`
    /// option). Returns `true` if the write happened, `false` if the key already
    /// existed. Backends SHOULD make this atomic; the default is a (racy)
    /// `exists`-then-`set` that can let two concurrent callers both win.
    async fn put_if_absent(
        &self,
        bucket: &str,
        key: &str,
        value: Vec<u8>,
    ) -> Result<bool, StoreError> {
        if self.exists(bucket, key).await? {
            return Ok(false);
        }
        self.set(bucket, key, value).await?;
        Ok(true)
    }

    /// Read a key's current value and version (the `cas.current` operation), or
    /// `None` if absent. Default: compare-and-swap is unsupported.
    async fn current(&self, _bucket: &str, _key: &str) -> Result<Option<Versioned>, StoreError> {
        Err(StoreError::Other(
            "compare-and-swap is not supported by this backend".to_string(),
        ))
    }

    /// Atomically write `value` to `key` iff every precondition in `guard` holds
    /// (the `cas.swap` operation). MUST be a single atomic compare-and-set
    /// against the backend, not a host-side read-then-write. Default:
    /// compare-and-swap is unsupported.
    async fn swap(
        &self,
        _bucket: &str,
        _key: &str,
        _value: Vec<u8>,
        _guard: CasGuard,
    ) -> Result<CasOutcome, StoreError> {
        Err(StoreError::Other(
            "compare-and-swap is not supported by this backend".to_string(),
        ))
    }
}

use crate::engine::ctx::ActiveCtx;

impl<'a> bindings::named_imports::wasi::keyvalue::store::Host for ActiveCtx<'a> {
    async fn open(
        &mut self,
        id: KvId,
        identifier: String,
    ) -> wasmtime::Result<Result<Resource<KvBucket>, StoreError>> {
        if let Err(e) = id.open(&identifier).await {
            return Ok(Err(e));
        }
        let bucket = KvBucket {
            backend: id,
            name: identifier,
        };
        Ok(Ok(self.table.push(bucket)?))
    }
}

impl<'a> bindings::named_imports::wasi::keyvalue::store::HostBucket for ActiveCtx<'a> {
    async fn get(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        key: String,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, StoreError>> {
        let b = self.table.get(&bucket)?;
        Ok(b.backend.get(&b.name, &key).await)
    }

    async fn set(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let b = self.table.get(&bucket)?;
        Ok(b.backend.set(&b.name, &key, value).await)
    }

    async fn delete(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        key: String,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let b = self.table.get(&bucket)?;
        Ok(b.backend.delete(&b.name, &key).await)
    }

    async fn exists(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        key: String,
    ) -> wasmtime::Result<Result<bool, StoreError>> {
        let b = self.table.get(&bucket)?;
        Ok(b.backend.exists(&b.name, &key).await)
    }

    async fn list_keys(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        cursor: Option<u64>,
    ) -> wasmtime::Result<Result<KeyResponse, StoreError>> {
        let b = self.table.get(&bucket)?;
        Ok(b.backend.list_keys(&b.name, cursor).await)
    }

    async fn drop(&mut self, _id: KvId, rep: Resource<KvBucket>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> bindings::named_imports::wasi::keyvalue::atomics::Host for ActiveCtx<'a> {
    async fn increment(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        key: String,
        delta: u64,
    ) -> wasmtime::Result<Result<u64, StoreError>> {
        // `wasi:keyvalue/atomics` is unsigned, but the shared `KvBackend` is
        // signed (for `wasmcloud:keyvalue`'s `s64`). Convert at the boundary: a
        // delta beyond `i64::MAX` or a counter that has gone negative is reported
        // as an error rather than silently wrapping.
        let b = self.table.get(&bucket)?;
        let Ok(delta) = i64::try_from(delta) else {
            return Ok(Err(StoreError::Other("delta exceeds i64::MAX".to_string())));
        };
        Ok(b.backend
            .increment(&b.name, &key, delta)
            .await
            .and_then(|v| {
                u64::try_from(v).map_err(|_| StoreError::Other("counter is negative".to_string()))
            }))
    }
}

impl<'a> bindings::named_imports::wasi::keyvalue::batch::Host for ActiveCtx<'a> {
    #[allow(clippy::type_complexity)]
    async fn get_many(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<Vec<Option<(String, Vec<u8>)>>, StoreError>> {
        let b = self.table.get(&bucket)?;
        Ok(b.backend.get_many(&b.name, keys).await)
    }

    async fn set_many(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let b = self.table.get(&bucket)?;
        Ok(b.backend.set_many(&b.name, key_values).await)
    }

    async fn delete_many(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let b = self.table.get(&bucket)?;
        Ok(b.backend.delete_many(&b.name, keys).await)
    }
}

const DEFAULT_BACKEND: &str = "in-memory";
const MULTIPLEXED_KEYVALUE_ID: &str = "wasi-keyvalue-multiplexed";

/// Page size for `list-keys` paging against the redis / NATS backends.
const LIST_KEYS_BATCH_SIZE: usize = 1000;

/// A keyvalue backend provider: a [`BackendProvider`] producing [`KvId`]s.
pub type KvProvider = dyn BackendProvider<KvId>;

mod filesystem;
mod in_memory;
mod nats;
mod redis;

pub use filesystem::{FilesystemBackend, FilesystemProvider};
pub use in_memory::{InMemoryBackend, InMemoryProvider};
pub use nats::{NatsBackend, NatsProvider};
pub use redis::{RedisBackend, RedisProvider};

/// A keyvalue [`HostPlugin`] that multiplexes `wasi:keyvalue` across backends
/// selected per `(implements ..)` import. Register the backend providers you
/// want to support via [`MultiplexedKeyValue::with_provider`].
pub struct MultiplexedKeyValue {
    mux: Multiplexer<KvId>,
}

impl Default for MultiplexedKeyValue {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiplexedKeyValue {
    pub fn new() -> Self {
        Self {
            mux: Multiplexer::new("wasi", "keyvalue", DEFAULT_BACKEND),
        }
    }

    /// Register a backend provider keyed by its `backend_type()`.
    pub fn with_provider(mut self, provider: Arc<KvProvider>) -> Self {
        self.mux = self.mux.with_provider(provider);
        self
    }

    /// Build the routing registry (host-interface name -> backend) for a
    /// component from its matched keyvalue host interfaces.
    pub async fn build_registry<'i>(
        &self,
        interfaces: impl IntoIterator<Item = &'i WitInterface>,
    ) -> anyhow::Result<HashMap<String, KvId>> {
        self.mux.build_registry(interfaces).await
    }
}

#[async_trait::async_trait]
impl HostPlugin for MultiplexedKeyValue {
    fn id(&self) -> &'static str {
        MULTIPLEXED_KEYVALUE_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasi:keyvalue/store,atomics,batch@0.2.0-draft",
            )]),
            ..Default::default()
        }
    }

    fn supports_named_instances(&self) -> bool {
        true
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !interfaces.contains("wasi", "keyvalue", &[]) {
            return Ok(());
        }

        let registry = self.build_registry(interfaces.iter()).await?;
        // Clone the component (cheap, Arc-backed) so the immutable borrow ends
        // before we take the mutable linker borrow.
        let component = item.component().clone();
        let linker = item.linker();

        bindings::named_imports::wasi::keyvalue::store::add_to_linker::<_, SharedCtx>(
            linker,
            &component,
            |name| self.mux.resolve(&registry, name),
            extract_active_ctx,
        )?;
        bindings::named_imports::wasi::keyvalue::atomics::add_to_linker::<_, SharedCtx>(
            linker,
            &component,
            |name| self.mux.resolve(&registry, name),
            extract_active_ctx,
        )?;
        bindings::named_imports::wasi::keyvalue::batch::add_to_linker::<_, SharedCtx>(
            linker,
            &component,
            |name| self.mux.resolve(&registry, name),
            extract_active_ctx,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv_iface(name: Option<&str>, backend: Option<&str>) -> WitInterface {
        let mut config = HashMap::new();
        if let Some(b) = backend {
            config.insert("backend".to_string(), b.to_string());
        }
        WitInterface {
            namespace: "wasi".to_string(),
            package: "keyvalue".to_string(),
            interfaces: ["store".to_string()].into_iter().collect(),
            version: None,
            config,
            name: name.map(String::from),
        }
    }

    #[tokio::test]
    async fn in_memory_backend_instances_are_isolated() {
        let a = InMemoryBackend::new();
        a.open("bucket").await.unwrap();
        a.set("bucket", "k", b"v1".to_vec()).await.unwrap();
        assert_eq!(a.get("bucket", "k").await.unwrap(), Some(b"v1".to_vec()));

        let b = InMemoryBackend::new();
        b.open("bucket").await.unwrap();
        assert_eq!(b.get("bucket", "k").await.unwrap(), None);
    }

    /// The decisive case: two named interfaces of the *same* backend type route
    /// to independent backends (postgres-per-team shape, proven with in-memory).
    #[tokio::test]
    async fn registry_routes_named_interfaces_to_distinct_backends() {
        let plugin = MultiplexedKeyValue::new().with_provider(Arc::new(InMemoryProvider));
        let interfaces = HashSet::from([
            kv_iface(Some("team-a"), Some("in-memory")),
            kv_iface(Some("team-b"), Some("in-memory")),
        ]);

        let registry = plugin.build_registry(&interfaces).await.unwrap();
        let a = registry.get("team-a").expect("team-a routed").clone();
        let b = registry.get("team-b").expect("team-b routed").clone();

        a.open("bucket").await.unwrap();
        a.set("bucket", "k", b"v".to_vec()).await.unwrap();
        b.open("bucket").await.unwrap();
        assert_eq!(b.get("bucket", "k").await.unwrap(), None, "backends leaked");
        assert_eq!(a.get("bucket", "k").await.unwrap(), Some(b"v".to_vec()));
    }

    /// The filesystem provider routes a named interface to an on-disk backend
    /// (the shared `FsKvStore`), proving it's wired into the multiplexer and the
    /// shared storage works through the `(implements ..)` path.
    #[tokio::test]
    async fn filesystem_backend_persists_through_the_multiplexer() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("wash-fs-kv-{}-{unique}", std::process::id()));

        let iface = WitInterface {
            namespace: "wasi".to_string(),
            package: "keyvalue".to_string(),
            interfaces: ["store".to_string()].into_iter().collect(),
            version: None,
            config: HashMap::from([
                ("backend".to_string(), "filesystem".to_string()),
                ("root".to_string(), dir.to_string_lossy().to_string()),
            ]),
            name: Some("team-fs".to_string()),
        };

        let plugin = MultiplexedKeyValue::new().with_provider(Arc::new(FilesystemProvider));
        let registry = plugin
            .build_registry(&HashSet::from([iface]))
            .await
            .unwrap();
        let backend = registry.get("team-fs").expect("team-fs routed").clone();

        backend.open("bucket").await.unwrap();
        backend.set("bucket", "k", b"v".to_vec()).await.unwrap();
        assert_eq!(
            backend.get("bucket", "k").await.unwrap(),
            Some(b"v".to_vec())
        );
        assert_eq!(backend.increment("bucket", "ctr", 5).await.unwrap(), 5);
        assert_eq!(backend.increment("bucket", "ctr", 3).await.unwrap(), 8);

        // It actually hit disk via the shared store.
        assert!(dir.join("bucket").join("k").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn build_registry_errors_on_unregistered_backend() {
        let plugin = MultiplexedKeyValue::new(); // no providers registered
        let interfaces = HashSet::from([kv_iface(Some("x"), Some("redis"))]);
        let err = plugin
            .build_registry(&interfaces)
            .await
            .err()
            .expect("expected error for unregistered backend");
        assert!(err.to_string().contains("redis"), "unexpected error: {err}");
    }
}
