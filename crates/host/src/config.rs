use std::collections::HashMap;

use anyhow::Context as _;
use tokio::sync::watch::{self, Receiver};

use crate::store::{DefaultStore, StoreManager};

#[async_trait::async_trait]
/// A trait for managing a config store which can be watched to receive updates to the config
pub trait ConfigManager: StoreManager {
    /// Watches a config by name and returns a receiver that will be notified when the config changes
    ///
    /// The default implementation returns a receiver that will never receive any updates.
    async fn watch(&self, name: &str) -> anyhow::Result<Receiver<HashMap<String, String>>> {
        let config = match self.get(name).await {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .context("Data corruption error, unable to decode data from store")?,
            Ok(None) => return Err(anyhow::anyhow!("Config {} does not exist", name)),
            Err(e) => return Err(anyhow::anyhow!("Error fetching config {}: {}", name, e)),
        };
        Ok(watch::channel(config).1)
    }
}

/// A default implementation of the config manager that does not watch for updates
impl ConfigManager for DefaultStore {}
