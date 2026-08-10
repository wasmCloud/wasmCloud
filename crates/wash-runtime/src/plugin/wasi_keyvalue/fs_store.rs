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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::plugin::lock_root;

/// Prefix for the transient files [`write_atomic`] creates next to their
/// destination. [`FsKvStore::list_keys`] filters entries with this prefix so a
/// concurrently-written value never shows up as a phantom key.
const TMP_PREFIX: &str = ".fskv-tmp-";

/// Process-wide discriminator for temp file names so concurrent writers to the
/// same key never collide on a temp path.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Replace `path`'s contents atomically: write a temp file in the same
/// directory (same filesystem), then rename it over the destination.
/// `rename(2)` replaces atomically on POSIX (and `std` maps the equivalent
/// replace semantics on Windows), so a concurrent reader observes either the
/// complete old value or the complete new value — never an empty or torn file,
/// which a plain `fs::write` (truncate, then write) exposes.
async fn write_atomic(path: &Path, value: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("key path has no parent directory"))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("key path has no file name"))?;
    let tmp = dir.join(format!(
        "{TMP_PREFIX}{}-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed),
        file_name.to_string_lossy(),
    ));
    tokio::fs::write(&tmp, value).await?;
    match tokio::fs::rename(&tmp, path).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // Best-effort cleanup; the rename error is the one worth reporting.
            let _ = tokio::fs::remove_file(&tmp).await;
            Err(e)
        }
    }
}

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
    /// Serializes the read-modify-write counter updates (`increment` /
    /// `increment_signed`) across every clone of this store. Without it two
    /// concurrent increments read the same current value and one update is
    /// lost — and a read racing the old non-atomic write could observe an
    /// empty file, parse it as 0, and silently reset the counter. Plain
    /// `set`/`get` don't take the lock; they are made safe by atomic replace
    /// in [`write_atomic`] instead.
    counter_lock: Arc<tokio::sync::Mutex<()>>,
}

impl FsKvStore {
    pub(crate) fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            counter_lock: Arc::new(tokio::sync::Mutex::new(())),
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
        write_atomic(&path, value).await.map_err(FsKvError::Io)
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
            // In-flight atomic writes must not surface as phantom keys.
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(TMP_PREFIX) {
                continue;
            }
            if remaining_skip != 0 {
                remaining_skip -= 1;
                continue;
            }
            if keys.len() >= batch {
                next_cursor = Some(skip as u64 + batch as u64);
                break;
            }
            keys.push(name);
        }
        Ok((keys, next_cursor))
    }

    /// Atomically increment a decimal counter stored as a string. The
    /// read-modify-write runs under [`Self::counter_lock`] and the write is an
    /// atomic replace, so concurrent increments never lose updates and
    /// concurrent readers never observe a torn value (which would parse as 0
    /// and silently reset the counter). A missing or unparseable value is
    /// treated as 0; the result is saturating so an overflow can't panic-trap
    /// a guest.
    pub(crate) async fn increment(
        &self,
        bucket: &str,
        key: &str,
        delta: u64,
    ) -> Result<u64, FsKvError> {
        let path = self.key_path(bucket, key)?;
        let _guard = self.counter_lock.lock().await;
        let current = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s.trim().parse::<u64>().unwrap_or(0),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(FsKvError::Io(e)),
        };
        let next = current.saturating_add(delta);
        write_atomic(&path, next.to_string().as_bytes())
            .await
            .map_err(FsKvError::Io)?;
        Ok(next)
    }

    /// Signed counterpart of [`Self::increment`] for the multiplexed
    /// `wasmcloud:keyvalue` backend, whose `atomics.increment` is `s64` (a
    /// negative `delta` decrements). The standalone `wasi:keyvalue` plugin keeps
    /// using the unsigned [`Self::increment`]; the two never share a store
    /// (separate roots), so the decimal encodings don't mix.
    #[cfg(feature = "wasm_component_model_implements")]
    pub(crate) async fn increment_signed(
        &self,
        bucket: &str,
        key: &str,
        delta: i64,
    ) -> Result<i64, FsKvError> {
        let path = self.key_path(bucket, key)?;
        let _guard = self.counter_lock.lock().await;
        let current = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s.trim().parse::<i64>().unwrap_or(0),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(FsKvError::Io(e)),
        };
        // Report overflow as an error (consistent with Redis `HINCRBY`) rather
        // than saturating or panicking.
        let next = current
            .checked_add(delta)
            .ok_or_else(|| FsKvError::Io(std::io::Error::other("counter overflow")))?;
        write_atomic(&path, next.to_string().as_bytes())
            .await
            .map_err(FsKvError::Io)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Concurrent increments must not lose updates and must never reset the
    /// counter. Before the counter was serialized (and its write made an
    /// atomic replace), two racing increments could read the same current
    /// value — and a read racing a truncate-then-write could observe an empty
    /// file, parse it as 0, and restart the counter from scratch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_increments_lose_no_updates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsKvStore::new(dir.path());
        store.create_bucket("b").await.ok().expect("bucket");

        const TASKS: u64 = 32;
        const PER_TASK: u64 = 25;
        let mut handles = Vec::new();
        for _ in 0..TASKS {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..PER_TASK {
                    store.increment("b", "counter", 1).await.ok().expect("incr");
                }
            }));
        }
        for h in handles {
            h.await.expect("task");
        }

        let final_value = store.increment("b", "counter", 0).await.ok().expect("read");
        assert_eq!(final_value, TASKS * PER_TASK);
    }

    /// A reader racing a writer must observe a complete old or complete new
    /// value — never a torn or empty one. With the previous plain
    /// truncate-then-write `set`, this test observed empty/partial reads.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn readers_never_observe_torn_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsKvStore::new(dir.path());
        store.create_bucket("b").await.ok().expect("bucket");

        let long: Vec<u8> = vec![b'A'; 64 * 1024];
        let short: Vec<u8> = vec![b'B'; 8];
        store.set("b", "k", &long).await.ok().expect("seed");

        let writer = {
            let store = store.clone();
            let (long, short) = (long.clone(), short.clone());
            tokio::spawn(async move {
                for i in 0..500u32 {
                    let v = if i % 2 == 0 { &short } else { &long };
                    store.set("b", "k", v).await.ok().expect("set");
                }
            })
        };

        let reader = {
            let store = store.clone();
            let (long, short) = (long.clone(), short.clone());
            tokio::spawn(async move {
                for _ in 0..500u32 {
                    let v = store
                        .get("b", "k")
                        .await
                        .ok()
                        .expect("get")
                        .expect("key must exist");
                    assert!(
                        v == long || v == short,
                        "torn read: {} bytes (expected {} or {})",
                        v.len(),
                        long.len(),
                        short.len()
                    );
                }
            })
        };

        writer.await.expect("writer");
        reader.await.expect("reader");
    }

    /// In-flight temp files must not surface through list_keys.
    #[tokio::test]
    async fn list_keys_hides_atomic_write_temp_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsKvStore::new(dir.path());
        store.create_bucket("b").await.ok().expect("bucket");
        store.set("b", "real-key", b"v").await.ok().expect("set");
        tokio::fs::write(
            dir.path().join("b").join(format!("{TMP_PREFIX}123-0-real-key")),
            b"partial",
        )
        .await
        .expect("plant temp file");

        let (keys, cursor) = store.list_keys("b", None, 10).await.ok().expect("list");
        assert_eq!(keys, vec!["real-key".to_string()]);
        assert!(cursor.is_none());
    }
}
