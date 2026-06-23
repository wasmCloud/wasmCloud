//! Shared filesystem-backed key-value storage.
//!
//! Buckets are subdirectories of a `root`; keys are files within them.
//! Path traversal is guarded by [`crate::plugin::lock_root`]. This is the single
//! storage implementation used by both the standalone [`FilesystemKeyValue`]
//! plugin (the unnamed/default `wasi:keyvalue` instance) and the multiplexed
//! [`FilesystemBackend`] (an `(implements ..)` named route) — each is a thin
//! adapter that maps [`FsKvError`] to its own interface error type.
//!
//! [`FilesystemKeyValue`]: super::filesystem::FilesystemKeyValue
//! [`FilesystemBackend`]: super::multiplexed::FilesystemBackend

use std::path::{Path, PathBuf};

use crate::plugin::lock_root;

/// An error from the shared filesystem key-value store. Adapters map this to
/// their own (`wasi:keyvalue/store`) error type.
pub(crate) enum FsKvError {
    /// A bucket or key identifier failed path-traversal validation.
    InvalidIdentifier,
    /// An underlying filesystem I/O error.
    Io(std::io::Error),
}

/// Filesystem key-value storage rooted at a directory.
#[derive(Clone)]
pub(crate) struct FsKvStore {
    root: PathBuf,
}

impl FsKvStore {
    pub(crate) fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Resolve (and traversal-check) a bucket's directory under `root`.
    fn bucket_root(&self, bucket: &str) -> Result<PathBuf, FsKvError> {
        lock_root(&self.root, bucket).map_err(|_| FsKvError::InvalidIdentifier)
    }

    /// Resolve (and traversal-check) a key's file under its bucket directory.
    fn key_path(&self, bucket: &str, key: &str) -> Result<PathBuf, FsKvError> {
        lock_root(self.bucket_root(bucket)?, key).map_err(|_| FsKvError::InvalidIdentifier)
    }

    /// Create the bucket directory (idempotent); also validates the identifier.
    pub(crate) async fn create_bucket(&self, bucket: &str) -> Result<(), FsKvError> {
        let root = self.bucket_root(bucket)?;
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(FsKvError::Io)
    }

    pub(crate) async fn get(&self, bucket: &str, key: &str) -> Result<Option<Vec<u8>>, FsKvError> {
        let path = self.key_path(bucket, key)?;
        match tokio::fs::read(path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(FsKvError::Io(e)),
        }
    }

    pub(crate) async fn set(&self, bucket: &str, key: &str, value: &[u8]) -> Result<(), FsKvError> {
        let path = self.key_path(bucket, key)?;
        tokio::fs::write(path, value).await.map_err(FsKvError::Io)
    }

    /// Delete a key. A missing key is a no-op (success).
    pub(crate) async fn delete(&self, bucket: &str, key: &str) -> Result<(), FsKvError> {
        let path = self.key_path(bucket, key)?;
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(FsKvError::Io(e)),
        }
    }

    pub(crate) async fn exists(&self, bucket: &str, key: &str) -> Result<bool, FsKvError> {
        let path = self.key_path(bucket, key)?;
        match tokio::fs::metadata(&path).await {
            // Directories are buckets, not keys.
            Ok(meta) => Ok(!meta.is_dir()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(FsKvError::Io(e)),
        }
    }

    /// List up to `batch` key names starting at `cursor`, returning the names
    /// and the next cursor (`Some` if more remain). A missing bucket directory
    /// yields an empty page.
    pub(crate) async fn list_keys(
        &self,
        bucket: &str,
        cursor: Option<u64>,
        batch: usize,
    ) -> Result<(Vec<String>, Option<u64>), FsKvError> {
        let root = self.bucket_root(bucket)?;
        let mut entries = match tokio::fs::read_dir(&root).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], None)),
            Err(e) => return Err(FsKvError::Io(e)),
        };

        let skip = cursor.unwrap_or(0) as usize;
        let mut remaining_skip = skip;
        let mut keys = Vec::new();
        let mut next_cursor = None;
        while let Some(entry) = entries.next_entry().await.map_err(FsKvError::Io)? {
            if remaining_skip != 0 {
                remaining_skip -= 1;
                continue;
            }
            if keys.len() >= batch {
                next_cursor = Some(skip as u64 + batch as u64);
                break;
            }
            keys.push(entry.file_name().to_string_lossy().to_string());
        }
        Ok((keys, next_cursor))
    }

    /// Atomically-ish increment a decimal counter stored as a string. A missing
    /// or unparseable value is treated as 0; the result is saturating so an
    /// overflow can't panic-trap a guest.
    pub(crate) async fn increment(
        &self,
        bucket: &str,
        key: &str,
        delta: u64,
    ) -> Result<u64, FsKvError> {
        let path = self.key_path(bucket, key)?;
        let current = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s.trim().parse::<u64>().unwrap_or(0),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(FsKvError::Io(e)),
        };
        let next = current.saturating_add(delta);
        tokio::fs::write(&path, next.to_string())
            .await
            .map_err(FsKvError::Io)?;
        Ok(next)
    }
}
