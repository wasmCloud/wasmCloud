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

use bytes::{Buf, Bytes};
use futures::stream::{FuturesOrdered, StreamExt};
use redis::AsyncCommands;
use tokio::sync::RwLock;
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
    async fn list_keys(&self, bucket: &str, cursor: Option<u64>)
    -> Result<KeyResponse, StoreError>;
    async fn increment(&self, bucket: &str, key: &str, delta: u64) -> Result<u64, StoreError>;
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
}

use crate::engine::ctx::ActiveCtx;

// ---- store interface -------------------------------------------------------

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

// ---- atomics interface -----------------------------------------------------

impl<'a> bindings::named_imports::wasi::keyvalue::atomics::Host for ActiveCtx<'a> {
    async fn increment(
        &mut self,
        _id: KvId,
        bucket: Resource<KvBucket>,
        key: String,
        delta: u64,
    ) -> wasmtime::Result<Result<u64, StoreError>> {
        let b = self.table.get(&bucket)?;
        Ok(b.backend.increment(&b.name, &key, delta).await)
    }
}

// ---- batch interface -------------------------------------------------------

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

// ---- in-memory backend (reference impl; no external infra) -----------------

/// An in-memory [`KvBackend`], used to prove the routing mechanism and for
/// tests. Each instance is an isolated store, so two named imports backed by
/// two `InMemoryBackend`s do not share data.
#[derive(Default)]
pub struct InMemoryBackend {
    buckets: RwLock<HashMap<String, HashMap<String, Vec<u8>>>>,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn missing(bucket: &str) -> StoreError {
        StoreError::Other(format!("bucket '{bucket}' does not exist"))
    }
}

#[async_trait::async_trait]
impl KvBackend for InMemoryBackend {
    async fn open(&self, identifier: &str) -> Result<(), StoreError> {
        self.buckets
            .write()
            .await
            .entry(identifier.to_string())
            .or_default();
        Ok(())
    }

    async fn get(&self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let store = self.buckets.read().await;
        let b = store.get(bucket).ok_or_else(|| Self::missing(bucket))?;
        Ok(b.get(key).cloned())
    }

    async fn set(&self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        let mut store = self.buckets.write().await;
        let b = store.get_mut(bucket).ok_or_else(|| Self::missing(bucket))?;
        b.insert(key.to_string(), value);
        Ok(())
    }

    async fn delete(&self, bucket: &str, key: &str) -> Result<(), StoreError> {
        let mut store = self.buckets.write().await;
        let b = store.get_mut(bucket).ok_or_else(|| Self::missing(bucket))?;
        b.remove(key);
        Ok(())
    }

    async fn exists(&self, bucket: &str, key: &str) -> Result<bool, StoreError> {
        let store = self.buckets.read().await;
        let b = store.get(bucket).ok_or_else(|| Self::missing(bucket))?;
        Ok(b.contains_key(key))
    }

    async fn list_keys(
        &self,
        bucket: &str,
        cursor: Option<u64>,
    ) -> Result<KeyResponse, StoreError> {
        const PAGE_SIZE: usize = 100;
        let store = self.buckets.read().await;
        let b = store.get(bucket).ok_or_else(|| Self::missing(bucket))?;
        let mut keys: Vec<String> = b.keys().cloned().collect();
        keys.sort();
        let start = cursor.unwrap_or(0) as usize;
        let end = std::cmp::min(start + PAGE_SIZE, keys.len());
        let page = keys.get(start..end).unwrap_or_default().to_vec();
        let next = (end < keys.len()).then_some(end as u64);
        Ok(KeyResponse {
            keys: page,
            cursor: next,
        })
    }

    async fn increment(&self, bucket: &str, key: &str, delta: u64) -> Result<u64, StoreError> {
        let mut store = self.buckets.write().await;
        let b = store.get_mut(bucket).ok_or_else(|| Self::missing(bucket))?;
        let current = match b.get(key) {
            Some(bytes) if bytes.len() == 8 => {
                // len checked == 8 by the guard, so copy into a fixed array
                // without a fallible conversion (the lib denies unwrap/expect).
                let mut arr = [0u8; 8];
                arr.copy_from_slice(bytes);
                u64::from_le_bytes(arr)
            }
            Some(bytes) => String::from_utf8_lossy(bytes).parse::<u64>().unwrap_or(0),
            None => 0,
        };
        let next = current.saturating_add(delta);
        b.insert(key.to_string(), next.to_le_bytes().to_vec());
        Ok(next)
    }

    async fn get_many(
        &self,
        bucket: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, StoreError> {
        let store = self.buckets.read().await;
        let b = store.get(bucket).ok_or_else(|| Self::missing(bucket))?;
        Ok(keys
            .into_iter()
            .map(|k| b.get(&k).cloned().map(|v| (k, v)))
            .collect())
    }

    async fn set_many(
        &self,
        bucket: &str,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), StoreError> {
        let mut store = self.buckets.write().await;
        let b = store.get_mut(bucket).ok_or_else(|| Self::missing(bucket))?;
        for (k, v) in key_values {
            b.insert(k, v);
        }
        Ok(())
    }

    async fn delete_many(&self, bucket: &str, keys: Vec<String>) -> Result<(), StoreError> {
        let mut store = self.buckets.write().await;
        let b = store.get_mut(bucket).ok_or_else(|| Self::missing(bucket))?;
        for k in keys {
            b.remove(&k);
        }
        Ok(())
    }
}

// ---- providers + plugin ----------------------------------------------------

const DEFAULT_BACKEND: &str = "in-memory";
const MULTIPLEXED_KEYVALUE_ID: &str = "wasi-keyvalue-multiplexed";

/// A keyvalue backend provider: a [`BackendProvider`] producing [`KvId`]s.
pub type KvProvider = dyn BackendProvider<KvId>;

/// In-memory provider. Each named interface gets its own isolated store.
#[derive(Default)]
pub struct InMemoryProvider;

#[async_trait::async_trait]
impl BackendProvider<KvId> for InMemoryProvider {
    fn backend_type(&self) -> &'static str {
        DEFAULT_BACKEND
    }

    async fn instantiate(&self, _config: &HashMap<String, String>) -> anyhow::Result<KvId> {
        Ok(Arc::new(InMemoryBackend::new()))
    }
}

// ---- redis backend ---------------------------------------------------------

const LIST_KEYS_BATCH_SIZE: usize = 1000;

/// A redis-backed [`KvBackend`]. The bucket identifier is used as a key prefix
/// (`{bucket}:{key}`). Holds a shared multiplexed connection (pooled per url by
/// the provider).
pub struct RedisBackend {
    conn: redis::aio::MultiplexedConnection,
}

impl RedisBackend {
    fn prefixed(bucket: &str, key: &str) -> String {
        format!("{bucket}:{key}")
    }

    fn err(e: impl std::fmt::Display) -> StoreError {
        StoreError::Other(format!("Redis error: {e}"))
    }
}

#[async_trait::async_trait]
impl KvBackend for RedisBackend {
    async fn open(&self, _identifier: &str) -> Result<(), StoreError> {
        // Redis namespaces by key prefix; there is no bucket to create.
        Ok(())
    }

    async fn get(&self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let mut conn = self.conn.clone();
        conn.get::<_, Option<Vec<u8>>>(Self::prefixed(bucket, key))
            .await
            .map_err(Self::err)
    }

    async fn set(&self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        let mut conn = self.conn.clone();
        conn.set::<_, _, ()>(Self::prefixed(bucket, key), value)
            .await
            .map_err(Self::err)
    }

    async fn delete(&self, bucket: &str, key: &str) -> Result<(), StoreError> {
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(Self::prefixed(bucket, key))
            .await
            .map_err(Self::err)
    }

    async fn exists(&self, bucket: &str, key: &str) -> Result<bool, StoreError> {
        let mut conn = self.conn.clone();
        conn.exists::<_, bool>(Self::prefixed(bucket, key))
            .await
            .map_err(Self::err)
    }

    async fn list_keys(
        &self,
        bucket: &str,
        cursor: Option<u64>,
    ) -> Result<KeyResponse, StoreError> {
        let mut conn = self.conn.clone();
        let pattern = Self::prefixed(bucket, "*");
        let (next, raw): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor.unwrap_or(0))
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(LIST_KEYS_BATCH_SIZE)
            .query_async(&mut conn)
            .await
            .map_err(Self::err)?;
        let prefix = pattern.strip_suffix('*').unwrap_or("");
        let keys = raw
            .into_iter()
            .filter_map(|k| k.strip_prefix(prefix).map(str::to_string))
            .collect();
        Ok(KeyResponse {
            keys,
            cursor: (next != 0).then_some(next),
        })
    }

    async fn increment(&self, bucket: &str, key: &str, delta: u64) -> Result<u64, StoreError> {
        let mut conn = self.conn.clone();
        let delta = i64::try_from(delta)
            .map_err(|_| StoreError::Other(format!("delta {delta} exceeds i64::MAX")))?;
        conn.incr::<_, _, i64>(Self::prefixed(bucket, key), delta)
            .await
            .map(|v| v as u64)
            .map_err(Self::err)
    }

    async fn get_many(
        &self,
        bucket: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, StoreError> {
        if keys.is_empty() {
            return Ok(vec![]);
        }
        let mut conn = self.conn.clone();
        let redis_keys: Vec<String> = keys.iter().map(|k| Self::prefixed(bucket, k)).collect();
        let values: Vec<Option<Vec<u8>>> =
            conn.mget(redis_keys.as_slice()).await.map_err(Self::err)?;
        Ok(keys
            .into_iter()
            .zip(values)
            .map(|(k, v)| v.map(|v| (k, v)))
            .collect())
    }

    async fn set_many(
        &self,
        bucket: &str,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), StoreError> {
        if key_values.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.clone();
        let pairs: Vec<(String, Vec<u8>)> = key_values
            .into_iter()
            .map(|(k, v)| (Self::prefixed(bucket, &k), v))
            .collect();
        conn.mset::<_, _, ()>(pairs.as_slice())
            .await
            .map_err(Self::err)
    }

    async fn delete_many(&self, bucket: &str, keys: Vec<String>) -> Result<(), StoreError> {
        if keys.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.clone();
        let redis_keys: Vec<String> = keys.iter().map(|k| Self::prefixed(bucket, k)).collect();
        conn.del::<_, ()>(redis_keys.as_slice())
            .await
            .map_err(Self::err)
    }
}

/// Provider for [`RedisBackend`], selected by `config.backend = "redis"`.
/// Requires `config.url` (e.g. `redis://127.0.0.1:6379`).
#[derive(Default)]
pub struct RedisProvider;

#[async_trait::async_trait]
impl BackendProvider<KvId> for RedisProvider {
    fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
        config.get("url").cloned()
    }
    fn backend_type(&self) -> &'static str {
        "redis"
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<KvId> {
        let url = config
            .get("url")
            .ok_or_else(|| anyhow::anyhow!("redis keyvalue backend requires a 'url' config"))?;
        let client = redis::Client::open(url.as_str())?;
        let conn = client.get_multiplexed_async_connection().await?;
        Ok(Arc::new(RedisBackend { conn }))
    }
}

// ---- NATS JetStream backend ------------------------------------------------

/// A NATS JetStream KV-backed [`KvBackend`]. Each bucket maps to a JetStream KV
/// store (which must already exist); store handles are cached by name.
pub struct NatsBackend {
    context: Arc<async_nats::jetstream::Context>,
    stores: RwLock<HashMap<String, async_nats::jetstream::kv::Store>>,
}

impl NatsBackend {
    fn err(e: impl std::fmt::Display) -> StoreError {
        StoreError::Other(format!("JetStream error: {e}"))
    }

    async fn store(&self, bucket: &str) -> Result<async_nats::jetstream::kv::Store, StoreError> {
        if let Some(s) = self.stores.read().await.get(bucket) {
            return Ok(s.clone());
        }
        let kv = self
            .context
            .get_key_value(bucket)
            .await
            .map_err(Self::err)?;
        self.stores
            .write()
            .await
            .insert(bucket.to_string(), kv.clone());
        Ok(kv)
    }
}

#[async_trait::async_trait]
impl KvBackend for NatsBackend {
    async fn open(&self, identifier: &str) -> Result<(), StoreError> {
        self.store(identifier).await.map(|_| ())
    }

    async fn get(&self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let s = self.store(bucket).await?;
        Ok(s.get(key).await.map_err(Self::err)?.map(|b| b.to_vec()))
    }

    async fn set(&self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        let s = self.store(bucket).await?;
        s.put(key.to_string(), value.into())
            .await
            .map_err(Self::err)?;
        Ok(())
    }

    async fn delete(&self, bucket: &str, key: &str) -> Result<(), StoreError> {
        let s = self.store(bucket).await?;
        s.delete(key).await.map_err(Self::err)?;
        Ok(())
    }

    async fn exists(&self, bucket: &str, key: &str) -> Result<bool, StoreError> {
        let s = self.store(bucket).await?;
        Ok(s.get(key).await.map_err(Self::err)?.is_some())
    }

    async fn list_keys(
        &self,
        bucket: &str,
        cursor: Option<u64>,
    ) -> Result<KeyResponse, StoreError> {
        let s = self.store(bucket).await?;
        let skip = cursor.unwrap_or(0) as usize;
        let mut stream = s
            .keys()
            .await
            .map_err(Self::err)?
            .skip(skip)
            .take(LIST_KEYS_BATCH_SIZE + 1)
            .boxed();
        let mut resp = KeyResponse {
            keys: vec![],
            cursor: None,
        };
        while let Some(Ok(key)) = stream.next().await {
            if resp.keys.len() >= LIST_KEYS_BATCH_SIZE {
                resp.cursor = Some(skip as u64 + LIST_KEYS_BATCH_SIZE as u64);
                break;
            }
            resp.keys.push(key);
        }
        Ok(resp)
    }

    async fn increment(&self, bucket: &str, key: &str, delta: u64) -> Result<u64, StoreError> {
        let s = self.store(bucket).await?;
        // Optimistic CAS, matching the standalone NATS backend (big-endian u64).
        let (revision, current) = match s.entry(key).await.map_err(Self::err)? {
            // Read the counter as a big-endian u64, tolerating a malformed
            // (non-8-byte) value as 0 rather than panicking: `Buf::get_u64`
            // traps the guest if the value has fewer than 8 bytes.
            Some(mut e) => {
                let current = if e.value.len() >= 8 {
                    e.value.get_u64()
                } else {
                    0
                };
                (Some(e.revision), current)
            }
            None => (None, 0),
        };
        // saturating, matching the in-memory/filesystem backends: a host-method
        // panic on overflow would become a wasmtime trap.
        let next = current.saturating_add(delta);
        let bytes = Bytes::from(next.to_be_bytes().to_vec());
        match revision {
            Some(rev) => s.update(key, bytes, rev).await.map_err(Self::err)?,
            None => s.put(key.to_string(), bytes).await.map_err(Self::err)?,
        };
        Ok(next)
    }

    async fn get_many(
        &self,
        bucket: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, StoreError> {
        let s = self.store(bucket).await?;
        FuturesOrdered::from_iter(keys.into_iter().map(|key| {
            let s = s.clone();
            async move {
                Ok(s.get(&key)
                    .await
                    .map_err(Self::err)?
                    .map(|b| (key, b.to_vec())))
            }
        }))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
    }

    async fn set_many(
        &self,
        bucket: &str,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), StoreError> {
        let s = self.store(bucket).await?;
        FuturesOrdered::from_iter(key_values.into_iter().map(|(key, value)| {
            let s = s.clone();
            async move {
                s.put(key, value.into())
                    .await
                    .map(|_| ())
                    .map_err(Self::err)
            }
        }))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
    }

    async fn delete_many(&self, bucket: &str, keys: Vec<String>) -> Result<(), StoreError> {
        let s = self.store(bucket).await?;
        FuturesOrdered::from_iter(keys.into_iter().map(|key| {
            let s = s.clone();
            async move { s.delete(&key).await.map(|_| ()).map_err(Self::err) }
        }))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
    }
}

/// Provider for [`NatsBackend`], selected by `config.backend = "nats"`. Requires
/// `config.url` (e.g. `nats://127.0.0.1:4222`).
#[derive(Default)]
pub struct NatsProvider;

#[async_trait::async_trait]
impl BackendProvider<KvId> for NatsProvider {
    fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
        config.get("url").cloned()
    }
    fn backend_type(&self) -> &'static str {
        "nats"
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<KvId> {
        let url = config
            .get("url")
            .ok_or_else(|| anyhow::anyhow!("nats keyvalue backend requires a 'url' config"))?;
        let client = async_nats::connect(url).await?;
        let context = async_nats::jetstream::new(client);
        Ok(Arc::new(NatsBackend {
            context: Arc::new(context),
            stores: RwLock::new(HashMap::new()),
        }))
    }
}

mod filesystem;
pub use filesystem::{FilesystemBackend, FilesystemProvider};

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
