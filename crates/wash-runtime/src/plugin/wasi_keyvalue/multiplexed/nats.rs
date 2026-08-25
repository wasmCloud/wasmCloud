//! NATS JetStream KV-backed [`KvBackend`] for the multiplexed keyvalue plugin.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Buf, Bytes};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::RwLock;

use crate::plugin::multiplex::BackendProvider;
use crate::plugin::wasi_keyvalue::nats_bucket::{BucketPolicy, OpenError};

use super::{
    CasGuard, CasOutcome, KeyResponse, KvBackend, KvId, LIST_KEYS_BATCH_SIZE, StoreError, Versioned,
};

/// How long a resolved bucket handle is reused before being looked up again.
///
/// A handle is a snapshot, not a session: `async_nats`'s `Store` carries the
/// stream's config from resolve time and reads `allow_direct` off it, so one
/// kept forever can route reads by a config the bucket no longer has.
/// Re-resolving also means a bucket deleted out of band stops looking present.
const STORE_TTL: Duration = Duration::from_secs(300);

/// Ceiling on cached handles. Bucket names come from guest identifiers, so the
/// map must not grow with them; past this the least recently resolved entries
/// are dropped and simply re-resolve on next use.
const STORE_CACHE_CAP: usize = 512;

/// A resolved handle and when it was resolved.
struct CachedStore<V> {
    store: V,
    resolved: Instant,
}

/// Bucket handles by physical name, with a TTL and a ceiling.
///
/// Generic over the handle so the expiry and ceiling can be tested without a
/// NATS connection; the backend instantiates it with `kv::Store`.
struct StoreCache<V = async_nats::jetstream::kv::Store> {
    entries: HashMap<String, CachedStore<V>>,
    ttl: Duration,
    cap: usize,
}

impl<V> Default for StoreCache<V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            ttl: STORE_TTL,
            cap: STORE_CACHE_CAP,
        }
    }
}

impl<V: Clone> StoreCache<V> {
    /// The handle for `physical`, if one was resolved recently enough.
    fn get(&self, physical: &str, now: Instant) -> Option<V> {
        self.entries
            .get(physical)
            .filter(|entry| now.duration_since(entry.resolved) < self.ttl)
            .map(|entry| entry.store.clone())
    }

    fn insert(&mut self, physical: String, store: V, now: Instant) {
        if self.entries.len() >= self.cap {
            let ttl = self.ttl;
            self.entries
                .retain(|_, entry| now.duration_since(entry.resolved) < ttl);
        }
        // Nothing expired, so make room by dropping the oldest resolves.
        while self.entries.len() >= self.cap {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.resolved)
                .map(|(name, _)| name.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            physical,
            CachedStore {
                store,
                resolved: now,
            },
        );
    }

    fn remove(&mut self, physical: &str) {
        self.entries.remove(physical);
    }
}

/// A NATS JetStream KV-backed [`KvBackend`]. A [`BucketPolicy`] maps the
/// identifier a guest opens onto the physical JetStream bucket and decides
/// whether a missing one may be created; resolved handles are cached by
/// physical name for [`STORE_TTL`].
///
/// A bucket deleted out of band is reported to the guest as `no-such-store`,
/// not as a transport error: an operation failure is checked against the
/// bucket before being classified. A bucket deleted and *recreated* under the
/// same name is simply an empty bucket — neither `wasi:keyvalue` nor
/// `wasmcloud:keyvalue` has a store generation to signal, and the other
/// backends (a redis prefix, a filesystem directory) behave the same way. The
/// one sharp edge is that JetStream revisions restart at 1, so a CAS version
/// token taken before a recreate is not comparable to one taken after; see
/// [`Versioned`].
pub struct NatsBackend {
    context: Arc<async_nats::jetstream::Context>,
    policy: BucketPolicy,
    stores: RwLock<StoreCache>,
}

impl NatsBackend {
    fn err(e: impl std::fmt::Display) -> StoreError {
        StoreError::Other(format!("JetStream error: {e}"))
    }

    fn open_err(e: OpenError) -> StoreError {
        match e {
            OpenError::NoSuchStore => StoreError::NoSuchStore,
            OpenError::Other(e) => StoreError::Other(e),
        }
    }

    /// Resolve a bucket for an operation. Look-up only: a bucket is created
    /// (when the policy allows) by `open` alone, so a stray `get` on an
    /// identifier that was never opened cannot mint a stream.
    async fn store(&self, bucket: &str) -> Result<async_nats::jetstream::kv::Store, StoreError> {
        let physical = self.policy.physical_name(bucket);
        if let Some(s) = self.stores.read().await.get(&physical, Instant::now()) {
            return Ok(s);
        }
        let kv = self
            .policy
            .get(&self.context, &physical)
            .await
            .map_err(Self::open_err)?;
        self.cache(physical, kv.clone()).await;
        Ok(kv)
    }

    async fn cache(&self, physical: String, kv: async_nats::jetstream::kv::Store) {
        self.stores
            .write()
            .await
            .insert(physical, kv, Instant::now());
    }

    /// Classify an operation failure.
    ///
    /// A JetStream operation against a deleted stream fails in a
    /// transport-shaped way rather than saying "no such store", so ask the
    /// bucket before deciding: if it is gone, drop the cached handle and
    /// answer `no-such-store` — the case a guest can actually act on — and
    /// otherwise keep the original error. The extra round trip is on the error
    /// path only.
    async fn classify(&self, bucket: &str, e: impl std::fmt::Display) -> StoreError {
        let physical = self.policy.physical_name(bucket);
        match self.policy.get(&self.context, &physical).await {
            Err(OpenError::NoSuchStore) => {
                self.stores.write().await.remove(&physical);
                StoreError::NoSuchStore
            }
            _ => Self::err(e),
        }
    }

    /// [`NatsBackend::classify`] applied to one operation's result.
    async fn check<T, E: std::fmt::Display>(
        &self,
        bucket: &str,
        result: Result<T, E>,
    ) -> Result<T, StoreError> {
        match result {
            Ok(value) => Ok(value),
            Err(e) => Err(self.classify(bucket, e).await),
        }
    }
}

#[async_trait::async_trait]
impl KvBackend for NatsBackend {
    /// Resolve the bucket, creating it if the policy allows.
    ///
    /// Always asks the policy rather than trusting a cached handle: `open` is
    /// where a guest learns whether its store exists, so answering from a
    /// handle cached before the bucket was deleted would report a bucket that
    /// is gone as present, and leave the guest to discover it on the next
    /// operation instead.
    async fn open(&self, identifier: &str) -> Result<(), StoreError> {
        let physical = self.policy.physical_name(identifier);
        let opened = self.policy.open(&self.context, identifier).await;
        match opened {
            Ok((kv, _outcome)) => {
                self.cache(physical, kv).await;
                Ok(())
            }
            Err(e) => {
                // Whatever was cached for this name is no longer trustworthy.
                self.stores.write().await.remove(&physical);
                Err(Self::open_err(e))
            }
        }
    }

    async fn get(&self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let s = self.store(bucket).await?;
        Ok(self
            .check(bucket, s.get(key).await)
            .await?
            .map(|b| b.to_vec()))
    }

    async fn set(&self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        let s = self.store(bucket).await?;
        self.check(bucket, s.put(key.to_string(), value.into()).await)
            .await?;
        Ok(())
    }

    async fn delete(&self, bucket: &str, key: &str) -> Result<(), StoreError> {
        let s = self.store(bucket).await?;
        self.check(bucket, s.delete(key).await).await?;
        Ok(())
    }

    async fn exists(&self, bucket: &str, key: &str) -> Result<bool, StoreError> {
        let s = self.store(bucket).await?;
        Ok(self.check(bucket, s.get(key).await).await?.is_some())
    }

    async fn list_keys(
        &self,
        bucket: &str,
        cursor: Option<u64>,
    ) -> Result<KeyResponse, StoreError> {
        let s = self.store(bucket).await?;
        let skip = cursor.unwrap_or(0) as usize;
        let mut stream = self
            .check(bucket, s.keys().await)
            .await?
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
            let (revision, current) = match self.check(bucket, s.entry(key).await).await? {
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
                    Err(e) => return Err(self.classify(bucket, e).await),
                },
                // `create` (not `put`) so a concurrent create is detected and
                // retried instead of clobbered.
                None => match s.create(key, bytes).await {
                    Ok(_) => return Ok(next),
                    Err(e) if e.kind() == CreateErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(self.classify(bucket, e).await),
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
        let results: Vec<Result<Option<(String, Vec<u8>)>, _>> =
            FuturesUnordered::from_iter(keys.into_iter().map(|key| {
                let s = s.clone();
                async move {
                    s.get(&key)
                        .await
                        .map(|value| value.map(|b| (key, b.to_vec())))
                }
            }))
            .collect()
            .await;
        self.check(bucket, results.into_iter().collect::<Result<Vec<_>, _>>())
            .await
    }

    async fn set_many(
        &self,
        bucket: &str,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), StoreError> {
        let s = self.store(bucket).await?;
        let results: Vec<Result<(), _>> =
            FuturesUnordered::from_iter(key_values.into_iter().map(|(key, value)| {
                let s = s.clone();
                async move { s.put(key, value.into()).await.map(|_| ()) }
            }))
            .collect()
            .await;
        self.check(bucket, results.into_iter().collect::<Result<Vec<_>, _>>())
            .await?;
        Ok(())
    }

    async fn delete_many(&self, bucket: &str, keys: Vec<String>) -> Result<(), StoreError> {
        let s = self.store(bucket).await?;
        let results: Vec<Result<(), _>> =
            FuturesUnordered::from_iter(keys.into_iter().map(|key| {
                let s = s.clone();
                async move { s.delete(&key).await.map(|_| ()) }
            }))
            .collect()
            .await;
        self.check(bucket, results.into_iter().collect::<Result<Vec<_>, _>>())
            .await?;
        Ok(())
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
            Err(e) => Err(self.classify(bucket, e).await),
        }
    }

    async fn current(&self, bucket: &str, key: &str) -> Result<Option<Versioned>, StoreError> {
        let s = self.store(bucket).await?;
        let entry = self.check(bucket, s.entry(key).await).await?;
        Ok(present(entry))
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
        let entry = self.check(bucket, s.entry(key).await).await?;
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
                let entry = self.check(bucket, s.entry(key).await).await?;
                Ok(CasOutcome::Stale(present(entry)))
            }
            Err(e) => Err(self.classify(bucket, e).await),
        }
    }
}

/// A read entry as a [`Versioned`] value, treating a delete/purge tombstone as
/// absent. Takes the entry rather than reading it so the caller classifies the
/// read's failure against the bucket.
fn present(entry: Option<async_nats::jetstream::kv::Entry>) -> Option<Versioned> {
    use async_nats::jetstream::kv::Operation;
    entry
        .filter(|e| matches!(e.operation, Operation::Put))
        .map(|e| Versioned {
            value: e.value.to_vec(),
            version: e.revision.to_string(),
        })
}

#[cfg(test)]
mod tests {
    //! The cache's expiry and ceiling, which need no NATS connection. How a
    //! deleted bucket is reported to a guest is covered against a real server
    //! in `tests/integration_keyvalue_multiplexed.rs`.

    use super::*;

    fn cache(ttl: Duration, cap: usize) -> StoreCache<String> {
        StoreCache {
            entries: HashMap::new(),
            ttl,
            cap,
        }
    }

    /// A handle is reused inside its TTL and re-resolved after it, so it
    /// cannot carry a stream config — or a deleted bucket's existence —
    /// indefinitely.
    #[test]
    fn a_handle_expires() {
        let start = Instant::now();
        let mut cache = cache(Duration::from_secs(300), 8);
        cache.insert("counters".to_string(), "handle".to_string(), start);

        assert_eq!(
            cache.get("counters", start + Duration::from_secs(299)),
            Some("handle".to_string())
        );
        assert_eq!(
            cache.get("counters", start + Duration::from_secs(300)),
            None
        );
    }

    /// An evicted handle is gone immediately — this is what a failed operation
    /// does once it learns the bucket is missing.
    #[test]
    fn remove_drops_a_handle() {
        let now = Instant::now();
        let mut cache = cache(Duration::from_secs(300), 8);
        cache.insert("counters".to_string(), "handle".to_string(), now);
        cache.remove("counters");
        assert_eq!(cache.get("counters", now), None);
    }

    /// Guest identifiers name the buckets, so the map is capped however many
    /// distinct ones arrive.
    #[test]
    fn the_cache_is_bounded() {
        let now = Instant::now();
        let mut cache = cache(Duration::from_secs(300), 4);
        for i in 0..100 {
            cache.insert(format!("bucket-{i}"), i.to_string(), now);
        }
        assert!(
            cache.entries.len() <= 4,
            "held {} entries, cap is 4",
            cache.entries.len()
        );
    }

    /// Expired entries are what a full cache drops first; a live handle
    /// survives.
    #[test]
    fn expiry_makes_room_before_live_handles_are_dropped() {
        let start = Instant::now();
        let mut cache = cache(Duration::from_secs(60), 2);
        cache.insert("old".to_string(), "old".to_string(), start);

        let later = start + Duration::from_secs(61);
        cache.insert("fresh".to_string(), "fresh".to_string(), later);
        cache.insert("newest".to_string(), "newest".to_string(), later);

        assert_eq!(cache.get("old", later), None, "the expired entry made room");
        assert_eq!(cache.get("fresh", later), Some("fresh".to_string()));
        assert_eq!(cache.get("newest", later), Some("newest".to_string()));
    }
}

/// Provider for [`NatsBackend`], selected by `config.backend = "nats"`. Requires
/// `config.url` (e.g. `nats://127.0.0.1:4222`).
///
/// The remaining config is the interface's [`BucketPolicy`]: `bucket`,
/// `bucket_prefix`, `create` (`never` — the default here — or `missing`), and
/// the settings a created bucket is given (`replicas`, `storage`, `max_age`,
/// `history`, `max_bytes`). Any key an interface leaves unset comes from the
/// embedder's [`NatsProvider::with_defaults`] policy, whose own default is
/// `create = never`: a guest's identifier cannot create JetStream streams, so
/// an operator declares the buckets and anything else is `no-such-store`.
#[derive(Default)]
pub struct NatsProvider {
    defaults: BucketPolicy,
}

impl NatsProvider {
    /// The bucket policy interfaces inherit from unless they configure their
    /// own. This is how a host's `--keyvalue-nats-*` flags reach a named
    /// import, and it is shared by the `wasi:keyvalue` and `wasmcloud:keyvalue`
    /// plugins, which register the same providers.
    pub fn with_defaults(defaults: BucketPolicy) -> Self {
        Self { defaults }
    }
}

#[async_trait::async_trait]
impl BackendProvider<KvId> for NatsProvider {
    /// Pooled per URL *and* resolved bucket policy: the backend carries its
    /// policy, so two interfaces on one NATS server may only share a backend
    /// when they resolve buckets the same way. The policy is fingerprinted
    /// after merging with the embedder's defaults, so what an interface
    /// inherits counts the same as what it spells out — including an empty
    /// `bucket_prefix` opting out of an inherited one.
    ///
    /// A config this provider cannot parse yields no pool key at all: it is
    /// about to fail in `instantiate`, and a placeholder key could collide
    /// with a valid one.
    fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
        let url = config.get("url")?;
        let policy = BucketPolicy::from_config(config, &self.defaults).ok()?;
        Some(format!("{url}\u{1}{}", policy.fingerprint()))
    }
    fn backend_type(&self) -> &'static str {
        "nats"
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<KvId> {
        let url = config
            .get("url")
            .ok_or_else(|| anyhow::anyhow!("nats keyvalue backend requires a 'url' config"))?;
        let policy = BucketPolicy::from_config(config, &self.defaults)?;
        let client = async_nats::connect(url).await?;
        let context = async_nats::jetstream::new(client);
        Ok(Arc::new(NatsBackend {
            context: Arc::new(context),
            policy,
            stores: RwLock::default(),
        }))
    }
}
