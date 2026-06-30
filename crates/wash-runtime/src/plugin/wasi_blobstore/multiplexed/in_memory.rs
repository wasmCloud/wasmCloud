//! In-memory [`BlobBackend`] for the multiplexed blobstore plugin.
//!
//! Used to prove the routing mechanism and for tests. Each instance is an
//! isolated store, so two named imports backed by two `InMemoryBackend`s do not
//! share data.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::plugin::multiplex::BackendProvider;

use super::{
    BlobBackend, BlobBackendError, BlobId, BlobResult, ContainerInfo, DEFAULT_BACKEND, ObjectInfo,
    clamp_range, now_secs,
};

#[derive(Clone, Debug)]
struct MemObject {
    data: Vec<u8>,
    created_at: u64,
}

#[derive(Default, Debug)]
struct MemContainer {
    created_at: u64,
    objects: HashMap<String, MemObject>,
}

/// An in-memory [`BlobBackend`]. Each instance is an isolated store.
#[derive(Default)]
pub struct InMemoryBackend {
    containers: RwLock<HashMap<String, MemContainer>>,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl BlobBackend for InMemoryBackend {
    async fn create_container(&self, name: &str) -> BlobResult<()> {
        let mut containers = self.containers.write().await;
        if containers.contains_key(name) {
            return Err(BlobBackendError::ContainerAlreadyExists(name.to_string()));
        }
        containers.insert(
            name.to_string(),
            MemContainer {
                created_at: now_secs(),
                objects: HashMap::new(),
            },
        );
        Ok(())
    }

    async fn get_container(&self, name: &str) -> BlobResult<()> {
        self.container_exists(name).await.and_then(|exists| {
            exists
                .then_some(())
                .ok_or_else(|| BlobBackendError::NoSuchContainer(name.to_string()))
        })
    }

    async fn delete_container(&self, name: &str) -> BlobResult<()> {
        self.containers.write().await.remove(name);
        Ok(())
    }

    async fn container_exists(&self, name: &str) -> BlobResult<bool> {
        Ok(self.containers.read().await.contains_key(name))
    }

    async fn container_info(&self, name: &str) -> BlobResult<ContainerInfo> {
        let containers = self.containers.read().await;
        let c = containers
            .get(name)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(name.to_string()))?;
        Ok(ContainerInfo {
            name: name.to_string(),
            created_at: c.created_at,
        })
    }

    async fn clear_container(&self, name: &str) -> BlobResult<()> {
        let mut containers = self.containers.write().await;
        let c = containers
            .get_mut(name)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(name.to_string()))?;
        c.objects.clear();
        Ok(())
    }

    async fn get_data(
        &self,
        container: &str,
        object: &str,
        start: u64,
        end: u64,
    ) -> BlobResult<Vec<u8>> {
        let containers = self.containers.read().await;
        let c = containers
            .get(container)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(container.to_string()))?;
        let o = c
            .objects
            .get(object)
            .ok_or_else(|| BlobBackendError::NoSuchObject(object.to_string()))?;
        let range = clamp_range(start, end, o.data.len());
        Ok(o.data.get(range).unwrap_or_default().to_vec())
    }

    async fn write_data(&self, container: &str, object: &str, data: Vec<u8>) -> BlobResult<()> {
        let mut containers = self.containers.write().await;
        let c = containers
            .get_mut(container)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(container.to_string()))?;
        c.objects.insert(
            object.to_string(),
            MemObject {
                data,
                created_at: now_secs(),
            },
        );
        Ok(())
    }

    async fn list_objects(&self, container: &str) -> BlobResult<Vec<String>> {
        let containers = self.containers.read().await;
        let c = containers
            .get(container)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(container.to_string()))?;
        Ok(c.objects.keys().cloned().collect())
    }

    async fn delete_object(&self, container: &str, object: &str) -> BlobResult<()> {
        let mut containers = self.containers.write().await;
        let c = containers
            .get_mut(container)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(container.to_string()))?;
        c.objects.remove(object);
        Ok(())
    }

    async fn delete_objects(&self, container: &str, objects: &[String]) -> BlobResult<()> {
        let mut containers = self.containers.write().await;
        let c = containers
            .get_mut(container)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(container.to_string()))?;
        for object in objects {
            c.objects.remove(object);
        }
        Ok(())
    }

    async fn has_object(&self, container: &str, object: &str) -> BlobResult<bool> {
        let containers = self.containers.read().await;
        let c = containers
            .get(container)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(container.to_string()))?;
        Ok(c.objects.contains_key(object))
    }

    async fn object_info(&self, container: &str, object: &str) -> BlobResult<ObjectInfo> {
        let containers = self.containers.read().await;
        let c = containers
            .get(container)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(container.to_string()))?;
        let o = c
            .objects
            .get(object)
            .ok_or_else(|| BlobBackendError::NoSuchObject(object.to_string()))?;
        Ok(ObjectInfo {
            name: object.to_string(),
            container: container.to_string(),
            created_at: o.created_at,
            size: o.data.len() as u64,
        })
    }

    async fn copy_object(
        &self,
        src_container: &str,
        src_object: &str,
        dest_container: &str,
        dest_object: &str,
    ) -> BlobResult<()> {
        let mut containers = self.containers.write().await;
        let src = containers
            .get(src_container)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(src_container.to_string()))?
            .objects
            .get(src_object)
            .ok_or_else(|| BlobBackendError::NoSuchObject(src_object.to_string()))?
            .data
            .clone();
        let dest = containers
            .get_mut(dest_container)
            .ok_or_else(|| BlobBackendError::NoSuchContainer(dest_container.to_string()))?;
        dest.objects.insert(
            dest_object.to_string(),
            MemObject {
                data: src,
                created_at: now_secs(),
            },
        );
        Ok(())
    }
}

/// In-memory provider. Each named interface gets its own isolated store.
#[derive(Default)]
pub struct InMemoryProvider;

#[async_trait::async_trait]
impl BackendProvider<BlobId> for InMemoryProvider {
    fn backend_type(&self) -> &'static str {
        DEFAULT_BACKEND
    }

    async fn instantiate(&self, _config: &HashMap<String, String>) -> anyhow::Result<BlobId> {
        Ok(Arc::new(InMemoryBackend::new()))
    }
}
