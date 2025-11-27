//! Module with structs for use in managing and accessing data used by various wasmCloud entities
use std::collections::HashMap;

use bytes::Bytes;
use tokio::sync::RwLock;
use tracing::instrument;

/// A store entry with value and revision for optimistic locking
#[derive(Clone, Debug)]
pub struct StoreEntry {
    /// The value stored in the entry
    pub value: Bytes,
    /// The revision number for optimistic concurrency control
    pub revision: u64,
}

#[async_trait::async_trait]
/// A trait for managing a store of data, such as a config store or a data store.
pub trait StoreManager: Send + Sync {
    /// Retrieves a value from the config store by key.
    async fn get(&self, key: &str) -> anyhow::Result<Option<Bytes>>;

    /// Inserts or updates a key-value pair in the config store.
    async fn put(&self, key: &str, value: Bytes) -> anyhow::Result<()>;

    /// Deletes a key from the config store.
    async fn del(&self, key: &str) -> anyhow::Result<()>;

    /// Retrieves an entry with value and revision for optimistic locking.
    async fn entry(&self, key: &str) -> anyhow::Result<Option<StoreEntry>>;

    /// Updates a value only if the revision matches (optimistic locking).
    async fn update(&self, key: &str, value: Bytes, expected_revision: u64) -> anyhow::Result<()>;
}

/// A struct that implements the StoreManager trait, storing data in an in-memory HashMap.
#[derive(Default)]
pub struct DefaultStore {
    store: RwLock<HashMap<String, StoreEntry>>,
}

#[async_trait::async_trait]
impl StoreManager for DefaultStore {
    #[instrument(skip(self))]
    async fn get(&self, key: &str) -> anyhow::Result<Option<Bytes>> {
        Ok(self.store.read().await.get(key).map(|e| e.value.clone()))
    }

    #[instrument(skip(self, value))]
    async fn put(&self, key: &str, value: Bytes) -> anyhow::Result<()> {
        let mut store = self.store.write().await;
        let new_revision = store.get(key).map(|e| e.revision + 1).unwrap_or(1);
        store.insert(
            key.to_string(),
            StoreEntry {
                value,
                revision: new_revision,
            },
        );
        Ok(())
    }

    #[instrument(skip(self))]
    async fn del(&self, key: &str) -> anyhow::Result<()> {
        self.store.write().await.remove(key);
        Ok(())
    }

    #[instrument(skip(self))]
    async fn entry(&self, key: &str) -> anyhow::Result<Option<StoreEntry>> {
        Ok(self.store.read().await.get(key).cloned())
    }

    #[instrument(skip(self, value))]
    async fn update(&self, key: &str, value: Bytes, expected_revision: u64) -> anyhow::Result<()> {
        let mut store = self.store.write().await;
        match store.get(key) {
            Some(entry) if entry.revision == expected_revision => {
                store.insert(
                    key.to_string(),
                    StoreEntry {
                        value,
                        revision: expected_revision + 1,
                    },
                );
                Ok(())
            }
            Some(entry) => {
                anyhow::bail!(
                    "revision mismatch: expected {}, got {}",
                    expected_revision,
                    entry.revision
                )
            }
            None => {
                anyhow::bail!("key not found: {}", key)
            }
        }
    }
}
