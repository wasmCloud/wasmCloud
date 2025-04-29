//! Module with structs for use in managing and accessing data used by various wasmCloud entities
use std::collections::HashMap;

use bytes::Bytes;
use tokio::sync::RwLock;
use tracing::instrument;

#[async_trait::async_trait]
/// A trait for managing a store of data, such as a config store or a data store.
pub trait StoreManager: Send + Sync {
    /// Retrieves a value from the config store by key.
    async fn get(&self, key: &str) -> anyhow::Result<Option<Bytes>>;

    /// Inserts or updates a key-value pair in the config store.
    async fn put(&self, key: &str, value: Bytes) -> anyhow::Result<()>;

    /// Deletes a key from the config store.
    async fn del(&self, key: &str) -> anyhow::Result<()>;
}

/// A struct that implements the StoreManager trait, storing data in an in-memory HashMap.
#[derive(Default)]
pub struct DefaultStore {
    store: RwLock<HashMap<String, Bytes>>,
}

#[async_trait::async_trait]
impl StoreManager for DefaultStore {
    #[instrument(skip(self))]
    async fn get(&self, key: &str) -> anyhow::Result<Option<Bytes>> {
        Ok(self.store.read().await.get(key).cloned())
    }

    #[instrument(skip(self, value))]
    async fn put(&self, key: &str, value: Bytes) -> anyhow::Result<()> {
        self.store.write().await.insert(key.to_string(), value);
        Ok(())
    }

    #[instrument(skip(self))]
    async fn del(&self, key: &str) -> anyhow::Result<()> {
        self.store.write().await.remove(key);
        Ok(())
    }
}
