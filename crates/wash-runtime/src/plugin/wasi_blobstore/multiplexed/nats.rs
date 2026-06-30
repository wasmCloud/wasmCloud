//! NATS JetStream object-store [`BlobBackend`] for the multiplexed blobstore
//! plugin.
//!
//! Each container maps to a JetStream object store (bucket) within one
//! JetStream context; the context (and thus connection) is shared across binds
//! with the same `config.url` via the multiplexer's connection pool. Object
//! bodies are buffered in memory — the host layers already buffer guest streams
//! before handing them off, so the backend reads/writes whole `Vec<u8>`s.

use std::collections::HashMap;
use std::sync::Arc;

use async_nats::jetstream::object_store::{self, ObjectStore};
use futures::StreamExt;
use tokio::io::AsyncReadExt;

use crate::plugin::multiplex::BackendProvider;

use super::{
    BlobBackend, BlobBackendError, BlobId, BlobResult, ContainerInfo, ObjectInfo, clamp_range,
};

/// A NATS JetStream object-store-backed [`BlobBackend`].
pub struct NatsBlobBackend {
    context: Arc<async_nats::jetstream::Context>,
}

impl NatsBlobBackend {
    fn err(e: impl std::fmt::Display) -> BlobBackendError {
        BlobBackendError::Other(format!("NATS object-store error: {e}"))
    }

    /// Resolve a container to its object store, mapping a missing bucket to
    /// `NoSuchContainer`.
    async fn store(&self, container: &str) -> BlobResult<ObjectStore> {
        self.context
            .get_object_store(container)
            .await
            .map_err(|_| BlobBackendError::NoSuchContainer(container.to_string()))
    }

    /// Collect the (non-deleted) object names in a store.
    async fn collect_names(store: &ObjectStore) -> BlobResult<Vec<String>> {
        let mut list = store.list().await.map_err(Self::err)?;
        let mut names = Vec::new();
        while let Some(item) = list.next().await {
            let info = item.map_err(Self::err)?;
            if !info.deleted {
                names.push(info.name);
            }
        }
        Ok(names)
    }

    /// Whether `object` currently exists. NATS object-store `delete` leaves a
    /// tombstone, and `info` still returns the deleted object's metadata with
    /// `deleted: true`, so presence must check that flag rather than just
    /// `info(..).is_ok()`.
    async fn present(store: &ObjectStore, object: &str) -> bool {
        matches!(store.info(object).await, Ok(info) if !info.deleted)
    }
}

#[async_trait::async_trait]
impl BlobBackend for NatsBlobBackend {
    async fn create_container(&self, name: &str) -> BlobResult<()> {
        if self.context.get_object_store(name).await.is_ok() {
            return Err(BlobBackendError::ContainerAlreadyExists(name.to_string()));
        }
        self.context
            .create_object_store(object_store::Config {
                bucket: name.to_string(),
                ..Default::default()
            })
            .await
            .map_err(Self::err)?;
        Ok(())
    }

    async fn get_container(&self, name: &str) -> BlobResult<()> {
        self.store(name).await.map(|_| ())
    }

    async fn delete_container(&self, name: &str) -> BlobResult<()> {
        // Idempotent: deleting a missing container is not an error.
        if self.context.get_object_store(name).await.is_ok() {
            self.context
                .delete_object_store(name)
                .await
                .map_err(Self::err)?;
        }
        Ok(())
    }

    async fn container_exists(&self, name: &str) -> BlobResult<bool> {
        Ok(self.context.get_object_store(name).await.is_ok())
    }

    async fn container_info(&self, name: &str) -> BlobResult<ContainerInfo> {
        self.store(name).await?;
        Ok(ContainerInfo {
            name: name.to_string(),
            // The object store does not expose a simple creation timestamp.
            created_at: 0,
        })
    }

    async fn clear_container(&self, name: &str) -> BlobResult<()> {
        let store = self.store(name).await?;
        for object in Self::collect_names(&store).await? {
            store.delete(&object).await.map_err(Self::err)?;
        }
        Ok(())
    }

    async fn get_data(
        &self,
        container: &str,
        object: &str,
        start: u64,
        end: u64,
    ) -> BlobResult<Vec<u8>> {
        let store = self.store(container).await?;
        let mut obj = store
            .get(object)
            .await
            .map_err(|_| BlobBackendError::NoSuchObject(object.to_string()))?;
        let mut buf = Vec::new();
        obj.read_to_end(&mut buf).await.map_err(Self::err)?;
        let range = clamp_range(start, end, buf.len());
        Ok(buf.get(range).unwrap_or_default().to_vec())
    }

    async fn write_data(&self, container: &str, object: &str, data: Vec<u8>) -> BlobResult<()> {
        let store = self.store(container).await?;
        let mut cursor = std::io::Cursor::new(data);
        store.put(object, &mut cursor).await.map_err(Self::err)?;
        Ok(())
    }

    async fn list_objects(&self, container: &str) -> BlobResult<Vec<String>> {
        let store = self.store(container).await?;
        Self::collect_names(&store).await
    }

    async fn delete_object(&self, container: &str, object: &str) -> BlobResult<()> {
        let store = self.store(container).await?;
        // Idempotent: deleting a missing (or already-deleted) object is not an
        // error, and avoids re-deleting a tombstone.
        if Self::present(&store, object).await {
            store.delete(object).await.map_err(Self::err)?;
        }
        Ok(())
    }

    async fn delete_objects(&self, container: &str, objects: &[String]) -> BlobResult<()> {
        let store = self.store(container).await?;
        for object in objects {
            if Self::present(&store, object).await {
                store.delete(object).await.map_err(Self::err)?;
            }
        }
        Ok(())
    }

    async fn has_object(&self, container: &str, object: &str) -> BlobResult<bool> {
        let store = self.store(container).await?;
        Ok(Self::present(&store, object).await)
    }

    async fn object_info(&self, container: &str, object: &str) -> BlobResult<ObjectInfo> {
        let store = self.store(container).await?;
        let info = store
            .info(object)
            .await
            .map_err(|_| BlobBackendError::NoSuchObject(object.to_string()))?;
        // `info` still returns a deleted object's (tombstone) metadata; treat
        // that as absent.
        if info.deleted {
            return Err(BlobBackendError::NoSuchObject(object.to_string()));
        }
        Ok(ObjectInfo {
            name: info.name,
            container: container.to_string(),
            created_at: 0,
            size: info.size as u64,
        })
    }

    async fn copy_object(
        &self,
        src_container: &str,
        src_object: &str,
        dest_container: &str,
        dest_object: &str,
    ) -> BlobResult<()> {
        let data = self
            .get_data(src_container, src_object, 0, u64::MAX)
            .await?;
        self.write_data(dest_container, dest_object, data).await
    }
}

/// Provider for [`NatsBlobBackend`], selected by `config.backend = "nats"`.
/// Requires `config.url` (e.g. `nats://127.0.0.1:4222`). Pooled by url, so
/// interfaces sharing a server share one connection.
#[derive(Default)]
pub struct NatsBlobProvider;

#[async_trait::async_trait]
impl BackendProvider<BlobId> for NatsBlobProvider {
    fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
        config.get("url").cloned()
    }

    fn backend_type(&self) -> &'static str {
        "nats"
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<BlobId> {
        let url = config
            .get("url")
            .ok_or_else(|| anyhow::anyhow!("nats blobstore backend requires a 'url' config"))?;
        let client = async_nats::connect(url).await?;
        let context = async_nats::jetstream::new(client);
        Ok(Arc::new(NatsBlobBackend {
            context: Arc::new(context),
        }))
    }
}
