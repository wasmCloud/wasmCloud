//! In-memory [`KvBackend`] for the multiplexed keyvalue plugin.
//!
//! Used to prove the routing mechanism and for tests. Each instance is an
//! isolated store, so two named imports backed by two `InMemoryBackend`s do not
//! share data. Every bucket carries a monotonic version counter so that
//! compare-and-swap is ABA-safe: a value that returns to identical bytes still
//! gets a fresh version, and `swap` does its compare-and-set under one write
//! lock (atomic in-process), so there is no lost-update race.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::plugin::multiplex::BackendProvider;

use super::{
    CasGuard, CasOutcome, DEFAULT_BACKEND, KeyResponse, KvBackend, KvId, StoreError, Versioned,
};

/// One stored value plus the version stamped at its last write.
#[derive(Clone)]
struct Entry {
    value: Vec<u8>,
    version: u64,
}

/// A bucket: its entries plus a monotonic version source. `next_version` only
/// ever increases (even across delete/re-create), so versions are unique within
/// the bucket and a key's version always changes on a write.
#[derive(Default)]
struct Bucket {
    entries: HashMap<String, Entry>,
    next_version: u64,
}

impl Bucket {
    /// Stamp `key` with `value` at a fresh version.
    fn put(&mut self, key: &str, value: Vec<u8>) -> u64 {
        self.next_version += 1;
        let version = self.next_version;
        self.entries
            .insert(key.to_string(), Entry { value, version });
        version
    }
}

/// An in-memory [`KvBackend`]. Each instance is an isolated store.
#[derive(Default)]
pub struct InMemoryBackend {
    buckets: RwLock<HashMap<String, Bucket>>,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn err_missing(bucket: &str) -> StoreError {
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
        let b = store.get(bucket).ok_or_else(|| Self::err_missing(bucket))?;
        Ok(b.entries.get(key).map(|e| e.value.clone()))
    }

    async fn set(&self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        let mut store = self.buckets.write().await;
        let b = store
            .get_mut(bucket)
            .ok_or_else(|| Self::err_missing(bucket))?;
        b.put(key, value);
        Ok(())
    }

    async fn delete(&self, bucket: &str, key: &str) -> Result<(), StoreError> {
        let mut store = self.buckets.write().await;
        let b = store
            .get_mut(bucket)
            .ok_or_else(|| Self::err_missing(bucket))?;
        b.entries.remove(key);
        Ok(())
    }

    async fn exists(&self, bucket: &str, key: &str) -> Result<bool, StoreError> {
        let store = self.buckets.read().await;
        let b = store.get(bucket).ok_or_else(|| Self::err_missing(bucket))?;
        Ok(b.entries.contains_key(key))
    }

    async fn list_keys(
        &self,
        bucket: &str,
        cursor: Option<u64>,
    ) -> Result<KeyResponse, StoreError> {
        const PAGE_SIZE: usize = 100;
        let store = self.buckets.read().await;
        let b = store.get(bucket).ok_or_else(|| Self::err_missing(bucket))?;
        let mut keys: Vec<String> = b.entries.keys().cloned().collect();
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

    async fn increment(&self, bucket: &str, key: &str, delta: i64) -> Result<i64, StoreError> {
        let mut store = self.buckets.write().await;
        let b = store
            .get_mut(bucket)
            .ok_or_else(|| Self::err_missing(bucket))?;
        let current = match b.entries.get(key) {
            Some(e) if e.value.len() == 8 => {
                // len checked == 8 by the guard, so copy into a fixed array
                // without a fallible conversion (the lib denies unwrap/expect).
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&e.value);
                i64::from_le_bytes(arr)
            }
            Some(e) => String::from_utf8_lossy(&e.value)
                .parse::<i64>()
                .unwrap_or(0),
            None => 0,
        };
        // Negative `delta` decrements. Report overflow as an error (consistent
        // with Redis `HINCRBY`) rather than saturating or panicking.
        let next = current
            .checked_add(delta)
            .ok_or_else(|| StoreError::Other("counter overflow".to_string()))?;
        b.put(key, next.to_le_bytes().to_vec());
        Ok(next)
    }

    async fn get_many(
        &self,
        bucket: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, StoreError> {
        let store = self.buckets.read().await;
        let b = store.get(bucket).ok_or_else(|| Self::err_missing(bucket))?;
        Ok(keys
            .into_iter()
            .map(|k| b.entries.get(&k).map(|e| (k, e.value.clone())))
            .collect())
    }

    async fn set_many(
        &self,
        bucket: &str,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), StoreError> {
        let mut store = self.buckets.write().await;
        let b = store
            .get_mut(bucket)
            .ok_or_else(|| Self::err_missing(bucket))?;
        for (k, v) in key_values {
            b.put(&k, v);
        }
        Ok(())
    }

    async fn delete_many(&self, bucket: &str, keys: Vec<String>) -> Result<(), StoreError> {
        let mut store = self.buckets.write().await;
        let b = store
            .get_mut(bucket)
            .ok_or_else(|| Self::err_missing(bucket))?;
        for k in keys {
            b.entries.remove(&k);
        }
        Ok(())
    }

    async fn put_if_absent(
        &self,
        bucket: &str,
        key: &str,
        value: Vec<u8>,
    ) -> Result<bool, StoreError> {
        // Check-and-insert under one write lock: atomic against concurrent
        // writers in-process, so two `if-not-exists` sets can't both win.
        let mut store = self.buckets.write().await;
        let b = store
            .get_mut(bucket)
            .ok_or_else(|| Self::err_missing(bucket))?;
        if b.entries.contains_key(key) {
            return Ok(false);
        }
        b.put(key, value);
        Ok(true)
    }

    async fn current(&self, bucket: &str, key: &str) -> Result<Option<Versioned>, StoreError> {
        let store = self.buckets.read().await;
        let b = store.get(bucket).ok_or_else(|| Self::err_missing(bucket))?;
        Ok(b.entries.get(key).map(|e| Versioned {
            value: e.value.clone(),
            version: e.version.to_string(),
        }))
    }

    async fn swap(
        &self,
        bucket: &str,
        key: &str,
        value: Vec<u8>,
        guard: CasGuard,
    ) -> Result<CasOutcome, StoreError> {
        // The whole compare-and-set runs under one write lock, so it is atomic
        // against concurrent writers (no lost update).
        let mut store = self.buckets.write().await;
        let b = store
            .get_mut(bucket)
            .ok_or_else(|| Self::err_missing(bucket))?;
        let current = b.entries.get(key);
        let stale = || {
            CasOutcome::Stale(current.map(|e| Versioned {
                value: e.value.clone(),
                version: e.version.to_string(),
            }))
        };
        if let Some(req) = &guard.require_version {
            // Version moved (or key absent) → precondition fails. ABA-safe: the
            // version is monotonic, so A → B → A does not reuse the old version.
            if current.map(|e| e.version.to_string()).as_deref() != Some(req.as_str()) {
                return Ok(stale());
            }
        }
        if let Some(req) = &guard.require_value
            && current.map(|e| e.value.as_slice()) != Some(req.as_slice())
        {
            return Ok(stale());
        }
        b.put(key, value);
        Ok(CasOutcome::Swapped)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    async fn version(be: &InMemoryBackend, key: &str) -> String {
        be.current("b", key).await.unwrap().unwrap().version
    }

    #[tokio::test]
    async fn cas_swap_is_aba_safe() {
        let be = InMemoryBackend::new();
        be.open("b").await.unwrap();
        be.set("b", "k", b"A".to_vec()).await.unwrap();
        let v1 = version(&be, "k").await;

        // A -> B -> A: the value returns to identical bytes, but the monotonic
        // version has moved, so a swap pinned to the original version is stale.
        be.set("b", "k", b"B".to_vec()).await.unwrap();
        be.set("b", "k", b"A".to_vec()).await.unwrap();

        let guard = CasGuard {
            require_version: Some(v1),
            require_value: None,
        };
        let out = be.swap("b", "k", b"C".to_vec(), guard).await.unwrap();
        assert!(
            matches!(out, CasOutcome::Stale(_)),
            "ABA must be detected: stale old version, not a false swap"
        );
        assert_eq!(be.get("b", "k").await.unwrap(), Some(b"A".to_vec()));

        // Swapping against the *current* version succeeds.
        let guard = CasGuard {
            require_version: Some(version(&be, "k").await),
            require_value: None,
        };
        let out = be.swap("b", "k", b"C".to_vec(), guard).await.unwrap();
        assert!(matches!(out, CasOutcome::Swapped));
        assert_eq!(be.get("b", "k").await.unwrap(), Some(b"C".to_vec()));
    }

    #[tokio::test]
    async fn cas_require_value() {
        let be = InMemoryBackend::new();
        be.open("b").await.unwrap();
        be.set("b", "k", b"A".to_vec()).await.unwrap();

        // Mismatched expected value → stale, carrying the current entry.
        let guard = CasGuard {
            require_version: None,
            require_value: Some(b"WRONG".to_vec()),
        };
        match be.swap("b", "k", b"C".to_vec(), guard).await.unwrap() {
            CasOutcome::Stale(Some(v)) => assert_eq!(v.value, b"A".to_vec()),
            other => panic!("expected stale, got {other:?}"),
        }
        assert_eq!(be.get("b", "k").await.unwrap(), Some(b"A".to_vec()));

        // Matching expected value → swapped.
        let guard = CasGuard {
            require_version: None,
            require_value: Some(b"A".to_vec()),
        };
        assert!(matches!(
            be.swap("b", "k", b"C".to_vec(), guard).await.unwrap(),
            CasOutcome::Swapped
        ));
    }

    #[tokio::test]
    async fn version_changes_on_every_write_including_recreate() {
        let be = InMemoryBackend::new();
        be.open("b").await.unwrap();
        be.set("b", "k", b"A".to_vec()).await.unwrap();
        let v1 = version(&be, "k").await;
        // delete then re-create with identical bytes must NOT reuse the version.
        be.delete("b", "k").await.unwrap();
        be.set("b", "k", b"A".to_vec()).await.unwrap();
        assert_ne!(
            v1,
            version(&be, "k").await,
            "version must not reset on recreate"
        );
    }

    #[tokio::test]
    async fn increment_is_signed_and_errors_on_overflow() {
        let be = InMemoryBackend::new();
        be.open("b").await.unwrap();
        // Signed: a negative delta decrements, and the counter may go below zero.
        assert_eq!(be.increment("b", "c", 5).await.unwrap(), 5);
        assert_eq!(be.increment("b", "c", -3).await.unwrap(), 2);
        assert_eq!(be.increment("b", "c", -10).await.unwrap(), -8);
        // Overflow is an error, not a silent saturate.
        be.set("b", "max", i64::MAX.to_le_bytes().to_vec())
            .await
            .unwrap();
        assert!(be.increment("b", "max", 1).await.is_err());
    }
}
