use crate::capability::{KeyValueAtomic, KeyValueEventual};

use core::sync::atomic::AtomicU64;

use std::collections::{hash_map, BTreeMap, HashMap};
use std::sync::atomic::Ordering;

use anyhow::{bail, Context};
use async_trait::async_trait;
use futures::stream;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::RwLock;
use tracing::instrument;
use wrpc_transport::IncomingInputStream;

/// Bucket entry
#[derive(Debug)]
pub enum Entry {
    /// Atomic number
    Atomic(AtomicU64),
    /// Byte blob
    Blob(Vec<u8>),
}

type Bucket = HashMap<String, Entry>;

/// In-memory [`KeyValueEventual`] and [`KeyValueAtomic`] implementation
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
impl KeyValueAtomic for KeyValue {
    async fn increment(&self, bucket: &str, key: String, delta: u64) -> anyhow::Result<u64> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?;
        if let Some(entry) = bucket.read().await.get(&key) {
            match entry {
                Entry::Atomic(value) => {
                    return Ok(value
                        .fetch_add(delta, Ordering::Relaxed)
                        .wrapping_add(delta));
                }
                Entry::Blob(_) => bail!("invalid entry type"),
            }
        }
        let mut bucket = bucket.write().await;
        match bucket.entry(key) {
            hash_map::Entry::Vacant(entry) => {
                entry.insert(Entry::Atomic(AtomicU64::new(delta)));
                Ok(delta)
            }
            hash_map::Entry::Occupied(entry) => match entry.get() {
                Entry::Atomic(value) => Ok(value
                    .fetch_add(delta, Ordering::Relaxed)
                    .wrapping_add(delta)),
                Entry::Blob(_) => bail!("invalid entry type"),
            },
        }
    }

    async fn compare_and_swap(
        &self,
        bucket: &str,
        key: String,
        old: u64,
        new: u64,
    ) -> anyhow::Result<bool> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?.read().await;
        match bucket.get(&key).context("key not found")? {
            Entry::Atomic(value) => Ok(value
                .compare_exchange(old, new, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok_and(|value| value == old)),
            Entry::Blob(_) => bail!("invalid entry type"),
        }
    }
}

#[async_trait]
impl KeyValueEventual for KeyValue {
    #[instrument]
    async fn get(&self, bucket: &str, key: String) -> anyhow::Result<Option<IncomingInputStream>> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?.read().await;
        let value = match bucket.get(&key) {
            None => return Ok(None),
            Some(Entry::Atomic(value)) => value.load(Ordering::Relaxed).to_string().into_bytes(),
            Some(Entry::Blob(value)) => value.clone(),
        };
        Ok(Some(Box::new(stream::iter([Ok(value.into())]))))
    }

    #[instrument(skip(value))]
    async fn set(
        &self,
        bucket: &str,
        key: String,
        mut value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()> {
        let mut buf = vec![];
        value
            .read_to_end(&mut buf)
            .await
            .context("failed to read value")?;
        let mut kv = self.0.write().await;
        let mut bucket = kv.entry(bucket.into()).or_default().write().await;
        bucket.insert(key, Entry::Blob(buf));
        Ok(())
    }

    #[instrument]
    async fn delete(&self, bucket: &str, key: String) -> anyhow::Result<()> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?;
        bucket.write().await.remove(&key).context("key not found")?;
        Ok(())
    }

    #[instrument]
    async fn exists(&self, bucket: &str, key: String) -> anyhow::Result<bool> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?.read().await;
        Ok(bucket.contains_key(&key))
    }
}
