use crate::capability::KeyValueReadWrite;

use std::collections::{hash_map, BTreeMap, HashMap};
use std::io::Cursor;

use anyhow::Context;
use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;
use tracing::instrument;

type Bucket = HashMap<String, Vec<u8>>;

/// In-memory [`KeyValueReadWrite`] implementation
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
impl KeyValueReadWrite for KeyValue {
    #[instrument]
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<(Box<dyn tokio::io::AsyncRead + Sync + Send + Unpin>, u64)> {
        let kv = self.0.read().await;
        let bucket = kv.get(bucket).context("bucket not found")?.read().await;
        let value = bucket.get(&key).context("key not found")?;
        let size = value
            .len()
            .try_into()
            .context("size does not fit in `u64`")?;
        Ok((Box::new(Cursor::new(value.clone())), size))
    }

    #[instrument(skip(value))]
    async fn set(
        &self,
        bucket: &str,
        key: String,
        mut value: Box<dyn tokio::io::AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()> {
        let mut buf = vec![];
        value
            .read_to_end(&mut buf)
            .await
            .context("failed to read value")?;
        let mut kv = self.0.write().await;
        let mut bucket = kv.entry(bucket.into()).or_default().write().await;
        bucket.insert(key, buf);
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
