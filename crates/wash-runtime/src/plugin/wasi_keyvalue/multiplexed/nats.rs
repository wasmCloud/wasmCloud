//! NATS JetStream KV-backed [`KvBackend`] for the multiplexed keyvalue plugin.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::{Buf, Bytes};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::RwLock;

use crate::plugin::multiplex::BackendProvider;

use super::{
    CasGuard, CasOutcome, KeyResponse, KvBackend, KvId, LIST_KEYS_BATCH_SIZE, StoreError, Versioned,
};

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
        use async_nats::jetstream::context::{
            GetStreamError, GetStreamErrorKind, KeyValueErrorKind,
        };
        use std::error::Error as _;
        let kv = self
            .context
            .get_key_value(bucket)
            .await
            .map_err(|e| match e.kind() {
                // An invalid name is never a real store.
                KeyValueErrorKind::InvalidStoreName => StoreError::NoSuchStore,
                // `GetBucket` wraps a `get_stream` failure: a missing/invalid
                // stream is `no-such-store`, but a `Request` (transport/timeout)
                // failure is a real error that must propagate so a guest can retry
                // instead of seeing "not found".
                KeyValueErrorKind::GetBucket => {
                    if matches!(
                        e.source().and_then(|s| s.downcast_ref::<GetStreamError>()),
                        Some(g) if g.kind() == GetStreamErrorKind::Request
                    ) {
                        Self::err(e)
                    } else {
                        StoreError::NoSuchStore
                    }
                }
                // A JetStream/transport failure is a real error, not "not found".
                KeyValueErrorKind::JetStream => Self::err(e),
            })?;
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

    async fn increment(&self, bucket: &str, key: &str, delta: i64) -> Result<i64, StoreError> {
        use async_nats::jetstream::kv::{CreateErrorKind, UpdateErrorKind};
        let s = self.store(bucket).await?;
        // Optimistic CAS retry loop (big-endian i64): read counter + revision,
        // compute, then conditionally write. A revision/exists conflict means a
        // concurrent writer won the race, so re-read and retry rather than
        // erroring. `atomics.increment` is defined to be atomic, so contention
        // must serialize, not fail.
        loop {
            let (revision, current) = match s.entry(key).await.map_err(Self::err)? {
                // Read the counter as a big-endian i64, tolerating a malformed
                // (non-8-byte) value as 0 rather than panicking: `Buf::get_i64`
                // traps if the value has fewer than 8 bytes.
                Some(mut e) => {
                    let current = if e.value.len() >= 8 {
                        e.value.get_i64()
                    } else {
                        0
                    };
                    (Some(e.revision), current)
                }
                None => (None, 0),
            };
            // Report overflow as an error (consistent with Redis `HINCRBY`)
            // rather than saturating or panicking.
            let next = current
                .checked_add(delta)
                .ok_or_else(|| StoreError::Other("counter overflow".to_string()))?;
            let bytes = Bytes::from(next.to_be_bytes().to_vec());
            match revision {
                // `update` is the atomic compare-and-set on the read revision.
                Some(rev) => match s.update(key, bytes, rev).await {
                    Ok(_) => return Ok(next),
                    Err(e) if e.kind() == UpdateErrorKind::WrongLastRevision => continue,
                    Err(e) => return Err(Self::err(e)),
                },
                // `create` (not `put`) so a concurrent create is detected and
                // retried instead of clobbered.
                None => match s.create(key, bytes).await {
                    Ok(_) => return Ok(next),
                    Err(e) if e.kind() == CreateErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(Self::err(e)),
                },
            }
        }
    }

    async fn get_many(
        &self,
        bucket: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, StoreError> {
        let s = self.store(bucket).await?;
        FuturesUnordered::from_iter(keys.into_iter().map(|key| {
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
        FuturesUnordered::from_iter(key_values.into_iter().map(|(key, value)| {
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
        FuturesUnordered::from_iter(keys.into_iter().map(|key| {
            let s = s.clone();
            async move { s.delete(&key).await.map(|_| ()).map_err(Self::err) }
        }))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
    }

    async fn put_if_absent(
        &self,
        bucket: &str,
        key: &str,
        value: Vec<u8>,
    ) -> Result<bool, StoreError> {
        use async_nats::jetstream::kv::CreateErrorKind;
        let s = self.store(bucket).await?;
        // `create` is the atomic insert-if-absent primitive.
        match s.create(key, Bytes::from(value)).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == CreateErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(Self::err(e)),
        }
    }

    async fn current(&self, bucket: &str, key: &str) -> Result<Option<Versioned>, StoreError> {
        let s = self.store(bucket).await?;
        Ok(present_entry(&s, key).await?)
    }

    async fn swap(
        &self,
        bucket: &str,
        key: &str,
        value: Vec<u8>,
        guard: CasGuard,
    ) -> Result<CasOutcome, StoreError> {
        use async_nats::jetstream::kv::Operation;
        let s = self.store(bucket).await?;

        // Read the current entry (with its revision) to evaluate preconditions.
        let entry = s.entry(key).await.map_err(Self::err)?;
        let revision = entry.as_ref().map(|e| e.revision);
        let current = entry
            .filter(|e| matches!(e.operation, Operation::Put))
            .map(|e| Versioned {
                value: e.value.to_vec(),
                version: e.revision.to_string(),
            });

        if let Some(req) = &guard.require_version
            && current.as_ref().map(|v| v.version.as_str()) != Some(req.as_str())
        {
            return Ok(CasOutcome::Stale(current));
        }
        if let Some(req) = &guard.require_value
            && current.as_ref().map(|v| v.value.as_slice()) != Some(req.as_slice())
        {
            return Ok(CasOutcome::Stale(current));
        }

        // A precondition can only pass for a present entry, so its revision is
        // known here; an absent key would already have returned `Stale` above.
        let Some(rev) = revision else {
            return Ok(CasOutcome::Stale(None));
        };
        // Atomic compare-and-set on the native revision: `update` succeeds only
        // if the revision has not moved since we read it. A `WrongLastRevision`
        // means a concurrent writer won the race — that, and only that, is a CAS
        // conflict, so we re-read and report the now-current entry as stale. Any
        // other error (network, auth, store deleted, ...) is a real failure and
        // must be propagated.
        use async_nats::jetstream::kv::UpdateErrorKind;
        match s.update(key, Bytes::from(value), rev).await {
            Ok(_) => Ok(CasOutcome::Swapped),
            Err(e) if e.kind() == UpdateErrorKind::WrongLastRevision => {
                Ok(CasOutcome::Stale(present_entry(&s, key).await?))
            }
            Err(e) => Err(Self::err(e)),
        }
    }
}

/// Read a key's current value + revision, treating a delete/purge tombstone as
/// absent.
async fn present_entry(
    store: &async_nats::jetstream::kv::Store,
    key: &str,
) -> Result<Option<Versioned>, StoreError> {
    use async_nats::jetstream::kv::Operation;
    Ok(store
        .entry(key)
        .await
        .map_err(NatsBackend::err)?
        .filter(|e| matches!(e.operation, Operation::Put))
        .map(|e| Versioned {
            value: e.value.to_vec(),
            version: e.revision.to_string(),
        }))
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
