use crate::capability::{keyvalue, KeyValueAtomics, KeyValueStore};

use core::sync::atomic::AtomicU64;

use std::collections::{hash_map, BTreeMap, HashMap};
use std::sync::atomic::Ordering;

use anyhow::{bail, Context};
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::instrument;

/// Bucket entry
#[derive(Debug)]
pub enum Entry {
    /// Atomic number
    Atomic(AtomicU64),
    /// Byte blob
    Blob(Vec<u8>),
}

type Bucket = HashMap<String, Entry>;

/// In-memory [`KeyValueStore`] and [`KeyValueAtomics`] implementation
#[derive(Debug)]
pub struct KeyValue(RwLock<HashMap<String, RwLock<Bucket>>>);

impl FromIterator<(String, RwLock<Bucket>)> for KeyValue {
    fn from_iter<T: IntoIterator<Item = (String, RwLock<Bucket>)>>(iter: T) -> Self {
        Self(RwLock::new(iter.into_iter().collect()))
    }
}

impl FromIterator<(String, Bucket)> for KeyValue {
    fn from_iter<T: IntoIterator<Item = (String, Bucket)>>(iter: T) -> Self {
        Self(RwLock::new(
            iter.into_iter().map(|(k, v)| (k, RwLock::new(v))).collect(),
        ))
    }
}

impl From<HashMap<String, Bucket>> for KeyValue {
    fn from(kv: HashMap<String, Bucket>) -> Self {
        kv.into_iter().collect()
    }
}

impl From<HashMap<String, RwLock<Bucket>>> for KeyValue {
    fn from(kv: HashMap<String, RwLock<Bucket>>) -> Self {
        kv.into_iter().collect()
    }
}

#[allow(clippy::implicit_hasher)]
impl From<KeyValue> for HashMap<String, Bucket> {
    fn from(KeyValue(kv): KeyValue) -> Self {
        kv.into_inner()
            .into_iter()
            .map(|(k, v)| (k, v.into_inner()))
            .collect()
    }
}

impl From<KeyValue> for BTreeMap<String, Bucket> {
    fn from(KeyValue(kv): KeyValue) -> Self {
        kv.into_inner()
            .into_iter()
            .map(|(k, v)| (k, v.into_inner()))
            .collect()
    }
}

impl IntoIterator for KeyValue {
    type Item = (String, Bucket);
    type IntoIter = hash_map::IntoIter<String, Bucket>;

    fn into_iter(self) -> Self::IntoIter {
        HashMap::from(self).into_iter()
    }
}

#[async_trait]
impl KeyValueAtomics for KeyValue {
    async fn increment(
        &self,
        bucket: &str,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64, keyvalue::store::Error>> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?;
        if let Some(entry) = bucket.read().await.get(&key) {
            match entry {
                Entry::Atomic(value) => {
                    return Ok(Ok(value
                        .fetch_add(delta, Ordering::Relaxed)
                        .wrapping_add(delta)));
                }
                Entry::Blob(_) => bail!("invalid entry type"),
            }
        }
        let mut bucket = bucket.write().await;
        match bucket.entry(key) {
            hash_map::Entry::Vacant(entry) => {
                entry.insert(Entry::Atomic(AtomicU64::new(delta)));
                Ok(Ok(delta))
            }
            hash_map::Entry::Occupied(entry) => match entry.get() {
                Entry::Atomic(value) => Ok(Ok(value
                    .fetch_add(delta, Ordering::Relaxed)
                    .wrapping_add(delta))),
                Entry::Blob(_) => bail!("invalid entry type"),
            },
        }
    }
}

#[async_trait]
impl KeyValueStore for KeyValue {
    #[instrument]
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, keyvalue::store::Error>> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?.read().await;
        Ok(Ok(match bucket.get(&key) {
            None => None,
            Some(Entry::Atomic(value)) => {
                Some(value.load(Ordering::Relaxed).to_string().into_bytes())
            }
            Some(Entry::Blob(value)) => Some(value.clone()),
        }))
    }

    #[instrument(skip(value))]
    async fn set(
        &self,
        bucket: &str,
        key: String,
        value: Vec<u8>,
    ) -> anyhow::Result<Result<(), keyvalue::store::Error>> {
        let mut kv = self.0.write().await;
        let mut bucket = kv.entry(bucket.into()).or_default().write().await;
        bucket.insert(key, Entry::Blob(value));
        Ok(Ok(()))
    }

    #[instrument]
    async fn delete(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<Result<(), keyvalue::store::Error>> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?;
        bucket.write().await.remove(&key).context("key not found")?;
        Ok(Ok(()))
    }

    #[instrument]
    async fn exists(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<Result<bool, keyvalue::store::Error>> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?.read().await;
        Ok(Ok(bucket.contains_key(&key)))
    }

    #[instrument]
    async fn list_keys(
        &self,
        bucket: &str,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<keyvalue::store::KeyResponse, keyvalue::store::Error>> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?.read().await;
        if cursor.is_some() {
            Ok(Err(keyvalue::store::Error::Other(
                "cursors not supported".into(),
            )))
        } else {
            Ok(Ok(keyvalue::store::KeyResponse {
                cursor: None,
                keys: bucket.keys().map(ToString::to_string).collect(),
            }))
        }
    }
}
