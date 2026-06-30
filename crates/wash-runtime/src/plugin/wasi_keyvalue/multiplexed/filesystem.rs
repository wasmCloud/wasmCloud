//! Filesystem backend for the multiplexed `wasi:keyvalue` plugin.
//!
//! A thin [`KvBackend`] adapter over the shared [`FsKvStore`] — the same
//! storage the standalone `FilesystemKeyValue` plugin uses.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::plugin::multiplex::BackendProvider;
use crate::plugin::wasi_keyvalue::fs_store::{FsKvError, FsKvStore};

use super::{KeyResponse, KvBackend, KvId, LIST_KEYS_BATCH_SIZE, StoreError};

/// A filesystem [`KvBackend`] rooted at a directory: buckets are subdirectories,
/// keys are files (path-traversal guarded). Backed by the shared [`FsKvStore`].
pub struct FilesystemBackend {
    store: FsKvStore,
}

impl FilesystemBackend {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            store: FsKvStore::new(root),
        }
    }
}

fn to_store_error(e: FsKvError) -> StoreError {
    match e {
        FsKvError::InvalidIdentifier => {
            StoreError::Other("invalid keyvalue identifier".to_string())
        }
        FsKvError::Io(e) => StoreError::Other(format!("Filesystem error: {e}")),
    }
}

#[async_trait::async_trait]
impl KvBackend for FilesystemBackend {
    async fn open(&self, identifier: &str) -> Result<(), StoreError> {
        self.store
            .create_bucket(identifier)
            .await
            .map_err(to_store_error)
    }

    async fn get(&self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.store.get(bucket, key).await.map_err(to_store_error)
    }

    async fn set(&self, bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        self.store
            .set(bucket, key, &value)
            .await
            .map_err(to_store_error)
    }

    async fn delete(&self, bucket: &str, key: &str) -> Result<(), StoreError> {
        self.store.delete(bucket, key).await.map_err(to_store_error)
    }

    async fn exists(&self, bucket: &str, key: &str) -> Result<bool, StoreError> {
        self.store.exists(bucket, key).await.map_err(to_store_error)
    }

    async fn list_keys(
        &self,
        bucket: &str,
        cursor: Option<u64>,
    ) -> Result<KeyResponse, StoreError> {
        let (keys, cursor) = self
            .store
            .list_keys(bucket, cursor, LIST_KEYS_BATCH_SIZE)
            .await
            .map_err(to_store_error)?;
        Ok(KeyResponse { keys, cursor })
    }

    async fn increment(&self, bucket: &str, key: &str, delta: i64) -> Result<i64, StoreError> {
        self.store
            .increment_signed(bucket, key, delta)
            .await
            .map_err(to_store_error)
    }

    async fn get_many(
        &self,
        bucket: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, StoreError> {
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            out.push(self.get(bucket, &key).await?.map(|v| (key, v)));
        }
        Ok(out)
    }

    async fn set_many(
        &self,
        bucket: &str,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), StoreError> {
        for (key, value) in key_values {
            self.set(bucket, &key, value).await?;
        }
        Ok(())
    }

    async fn delete_many(&self, bucket: &str, keys: Vec<String>) -> Result<(), StoreError> {
        for key in keys {
            self.delete(bucket, &key).await?;
        }
        Ok(())
    }
}

/// Provider for [`FilesystemBackend`], selected by `config.backend =
/// "filesystem"`. Requires `config.root` (the directory under which buckets
/// live).
#[derive(Default)]
pub struct FilesystemProvider;

#[async_trait::async_trait]
impl BackendProvider<KvId> for FilesystemProvider {
    fn backend_type(&self) -> &'static str {
        "filesystem"
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<KvId> {
        let root = config.get("root").ok_or_else(|| {
            anyhow::anyhow!("filesystem keyvalue backend requires a 'root' config")
        })?;
        Ok(Arc::new(FilesystemBackend::new(root)))
    }
}
