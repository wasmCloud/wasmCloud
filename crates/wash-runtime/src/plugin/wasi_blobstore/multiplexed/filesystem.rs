//! Filesystem [`BlobBackend`] for the multiplexed blobstore plugin.
//!
//! Rooted at a directory: containers are subdirectories, objects are files
//! (path-traversal guarded via [`lock_root`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::plugin::lock_root;
use crate::plugin::multiplex::BackendProvider;

use super::{
    BlobBackend, BlobBackendError, BlobId, BlobResult, ContainerInfo, ObjectInfo, clamp_range,
};

/// A filesystem [`BlobBackend`] rooted at a directory.
pub struct FilesystemBackend {
    root: PathBuf,
}

impl FilesystemBackend {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    fn container_path(&self, container: &str) -> BlobResult<PathBuf> {
        lock_root(&self.root, container).map_err(|e| BlobBackendError::Other(e.to_string()))
    }

    fn object_path(&self, container: &str, object: &str) -> BlobResult<PathBuf> {
        let dir = self.container_path(container)?;
        lock_root(dir, object).map_err(|e| BlobBackendError::Other(e.to_string()))
    }

    async fn created_at(path: &Path) -> u64 {
        match tokio::fs::metadata(path).await.and_then(|m| m.created()) {
            Ok(t) => t
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Err(_) => 0,
        }
    }
}

#[async_trait::async_trait]
impl BlobBackend for FilesystemBackend {
    async fn create_container(&self, name: &str) -> BlobResult<()> {
        let path = self.container_path(name)?;
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err(BlobBackendError::ContainerAlreadyExists(name.to_string()));
        }
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(BlobBackendError::other)
    }

    async fn get_container(&self, name: &str) -> BlobResult<()> {
        self.container_exists(name).await.and_then(|exists| {
            exists
                .then_some(())
                .ok_or_else(|| BlobBackendError::NoSuchContainer(name.to_string()))
        })
    }

    async fn delete_container(&self, name: &str) -> BlobResult<()> {
        let path = self.container_path(name)?;
        match tokio::fs::remove_dir_all(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(BlobBackendError::other(e)),
        }
    }

    async fn container_exists(&self, name: &str) -> BlobResult<bool> {
        let path = self.container_path(name)?;
        Ok(tokio::fs::try_exists(&path).await.unwrap_or(false))
    }

    async fn container_info(&self, name: &str) -> BlobResult<ContainerInfo> {
        let path = self.container_path(name)?;
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err(BlobBackendError::NoSuchContainer(name.to_string()));
        }
        Ok(ContainerInfo {
            name: name.to_string(),
            created_at: Self::created_at(&path).await,
        })
    }

    async fn clear_container(&self, name: &str) -> BlobResult<()> {
        let path = self.container_path(name)?;
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err(BlobBackendError::NoSuchContainer(name.to_string()));
        }
        // Remove the whole tree and recreate the (now empty) container, so that
        // objects written under nested keys (e.g. `a/b`) are cleared too.
        tokio::fs::remove_dir_all(&path)
            .await
            .map_err(BlobBackendError::other)?;
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(BlobBackendError::other)?;
        Ok(())
    }

    async fn get_data(
        &self,
        container: &str,
        object: &str,
        start: u64,
        end: u64,
    ) -> BlobResult<Vec<u8>> {
        let path = self.object_path(container, object)?;
        let data = match tokio::fs::read(&path).await {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(BlobBackendError::NoSuchObject(object.to_string()));
            }
            Err(e) => return Err(BlobBackendError::other(e)),
        };
        let range = clamp_range(start, end, data.len());
        Ok(data.get(range).unwrap_or_default().to_vec())
    }

    async fn write_data(&self, container: &str, object: &str, data: Vec<u8>) -> BlobResult<()> {
        if !self.container_exists(container).await? {
            return Err(BlobBackendError::NoSuchContainer(container.to_string()));
        }
        let path = self.object_path(container, object)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(BlobBackendError::other)?;
        }
        tokio::fs::write(&path, data)
            .await
            .map_err(BlobBackendError::other)
    }

    async fn list_objects(&self, container: &str) -> BlobResult<Vec<String>> {
        let root = self.container_path(container)?;
        if !tokio::fs::try_exists(&root).await.unwrap_or(false) {
            return Err(BlobBackendError::NoSuchContainer(container.to_string()));
        }
        // Walk the container tree so objects written under nested keys (e.g.
        // `a/b`, stored as a file `a/b`) are listed, matching the flat-key
        // in-memory and NATS backends. Each name is the file's path relative to
        // the container root, '/'-joined.
        let mut names = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&dir)
                .await
                .map_err(BlobBackendError::other)?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(BlobBackendError::other)?
            {
                let file_type = entry.file_type().await.map_err(BlobBackendError::other)?;
                let entry_path = entry.path();
                if file_type.is_dir() {
                    stack.push(entry_path);
                } else if file_type.is_file() {
                    let Ok(rel) = entry_path.strip_prefix(&root) else {
                        continue;
                    };
                    let name = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    names.push(name);
                }
            }
        }
        Ok(names)
    }

    async fn delete_object(&self, container: &str, object: &str) -> BlobResult<()> {
        let path = self.object_path(container, object)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(BlobBackendError::other(e)),
        }
    }

    async fn delete_objects(&self, container: &str, objects: &[String]) -> BlobResult<()> {
        for object in objects {
            self.delete_object(container, object).await?;
        }
        Ok(())
    }

    async fn has_object(&self, container: &str, object: &str) -> BlobResult<bool> {
        let path = self.object_path(container, object)?;
        // Only a regular file is an object. A directory here is an intermediate
        // path component of a nested key (e.g. `a` exists because `a/b` was
        // written) — not an object named `a`.
        Ok(tokio::fs::metadata(&path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false))
    }

    async fn object_info(&self, container: &str, object: &str) -> BlobResult<ObjectInfo> {
        let path = self.object_path(container, object)?;
        let meta = match tokio::fs::metadata(&path).await {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(BlobBackendError::NoSuchObject(object.to_string()));
            }
            Err(e) => return Err(BlobBackendError::other(e)),
        };
        // A directory (nested-key intermediate) is not an object.
        if !meta.is_file() {
            return Err(BlobBackendError::NoSuchObject(object.to_string()));
        }
        Ok(ObjectInfo {
            name: object.to_string(),
            container: container.to_string(),
            created_at: Self::created_at(&path).await,
            size: meta.len(),
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

/// Provider for [`FilesystemBackend`], selected by `config.backend =
/// "filesystem"`. Requires `config.root` (the directory under which containers
/// live).
#[derive(Default)]
pub struct FilesystemProvider;

#[async_trait::async_trait]
impl BackendProvider<BlobId> for FilesystemProvider {
    fn backend_type(&self) -> &'static str {
        "filesystem"
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<BlobId> {
        let root = config.get("root").ok_or_else(|| {
            anyhow::anyhow!("filesystem blobstore backend requires a 'root' config")
        })?;
        Ok(Arc::new(FilesystemBackend::new(root)))
    }
}
