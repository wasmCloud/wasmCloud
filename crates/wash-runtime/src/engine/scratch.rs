//! Where `emptyDir` volumes live, and how they are reclaimed.
//!
//! Scratch lives under one root the host owns, never loose in `$TMPDIR`: one
//! directory per host process, holding one directory per workload. That makes
//! the set on disk enumerable, which is what reclaiming it needs.
//!
//! Two mechanisms reclaim it, because neither covers every failure alone:
//!
//! - A workload's directory is removed when the workload is released, or else
//!   when its last handle drops, which covers a start that failed before it was
//!   bound.
//! - A host that crashed runs no teardown. Each host holds an exclusive lock on
//!   a file in its own directory for as long as it runs, and the kernel drops
//!   that lock when the process dies. When a host builds its engine it removes
//!   every other host directory whose lock it can take. A host's lock file is
//!   locked before it is published under its name, so no sweep ever finds a
//!   starting host's lock free.
//!
//! The sweep deletes only what it can prove is dead. A directory whose lock
//! cannot be taken, whose lock file is missing, or which cannot be read is kept:
//! a directory lost to a sweep is another tenant's data, and one kept is disk.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Context as _;

const LOCK_FILE: &str = ".lock";
const HOST_DIR_PREFIX: &str = "host-";

/// This host's directory under the scratch root, held for the host's lifetime.
#[derive(Debug)]
pub(crate) struct ScratchRoot {
    dir: PathBuf,
    /// Held, never read: the lock is the host's liveness marker.
    _lock: File,
}

impl ScratchRoot {
    /// Claim a directory under `root`, after sweeping what crashed hosts left.
    pub(crate) fn open(root: &Path) -> anyhow::Result<Arc<Self>> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("failed to create scratch root {}", root.display()))?;
        sweep(root);
        let dir = tempfile::Builder::new()
            .prefix(HOST_DIR_PREFIX)
            .tempdir_in(root)
            .with_context(|| format!("failed to create a host directory in {}", root.display()))?
            .keep();
        let lock = tempfile::Builder::new()
            .tempfile_in(&dir)
            .with_context(|| format!("failed to create a lock file in {}", dir.display()))?;
        lock.as_file()
            .try_lock()
            .map_err(|e| anyhow::anyhow!("failed to lock {}: {e}", lock.path().display()))?;
        let lock = lock
            .persist(dir.join(LOCK_FILE))
            .map_err(|e| e.error)
            .with_context(|| format!("failed to publish the lock in {}", dir.display()))?;
        Ok(Arc::new(Self { dir, _lock: lock }))
    }

    /// A new directory for one workload's `emptyDir` volumes.
    pub(crate) fn workload_dir(self: &Arc<Self>) -> anyhow::Result<Arc<ScratchDir>> {
        let path = tempfile::Builder::new()
            .prefix("workload-")
            .tempdir_in(&self.dir)
            .with_context(|| format!("failed to create scratch in {}", self.dir.display()))?
            .keep();
        Ok(Arc::new(ScratchDir {
            path,
            removed: Arc::default(),
            root: Arc::clone(self),
        }))
    }
}

impl Drop for ScratchRoot {
    /// A clean shutdown leaves nothing for the next host's sweep.
    fn drop(&mut self) {
        remove(&self.dir);
    }
}

/// One workload's scratch: removed when the workload is released, or when the
/// last handle to it drops.
#[derive(Debug)]
pub(crate) struct ScratchDir {
    path: PathBuf,
    /// Whether a removal succeeded; the name may since belong to another
    /// workload. Held over each removal, the drop's included, so two never race.
    removed: Arc<Mutex<bool>>,
    /// Keeps the host directory, and its lock, alive while this lives.
    root: Arc<ScratchRoot>,
}

impl ScratchDir {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the directory now, whoever else holds it (an open handle keeps
    /// working on Unix), and say whether it is gone. Only the first success removes.
    pub(crate) fn remove(&self) -> bool {
        remove_once(&self.removed, &self.path)
    }

    /// [`Self::remove`] on the blocking pool, so a large directory never holds
    /// an async worker. It finishes even when the caller is cancelled.
    pub(crate) async fn remove_on_blocking_pool(self: &Arc<Self>) -> bool {
        let scratch = Arc::clone(self);
        match tokio::task::spawn_blocking(move || scratch.remove()).await {
            Ok(removed) => removed,
            Err(e) => {
                tracing::warn!(path = %self.path.display(), err = %e, "failed to remove scratch");
                false
            }
        }
    }

    /// The lock every removal takes; holding it stands in for a slow removal.
    #[cfg(test)]
    pub(crate) fn removal_lock(&self) -> Arc<Mutex<bool>> {
        Arc::clone(&self.removed)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if self.removed.try_lock().is_ok_and(|removed| *removed) {
            return;
        }
        let removal = PendingRemoval {
            removed: Arc::clone(&self.removed),
            path: std::mem::take(&mut self.path),
            _root: Arc::clone(&self.root),
        };
        // Off the async worker when there is one. A task the pool never runs is
        // dropped, and the removal then runs inline.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(move || drop(removal));
            }
            Err(_) => drop(removal),
        }
    }
}

/// A dropped scratch's removal, run when this drops. It holds the root, so the
/// host directory outlives the removal.
struct PendingRemoval {
    removed: Arc<Mutex<bool>>,
    path: PathBuf,
    _root: Arc<ScratchRoot>,
}

impl Drop for PendingRemoval {
    fn drop(&mut self) {
        remove_once(&self.removed, &self.path);
    }
}

/// Remove `path` unless a removal already succeeded, and say whether it is gone.
fn remove_once(removed: &Mutex<bool>, path: &Path) -> bool {
    let mut removed = removed.lock().unwrap_or_else(PoisonError::into_inner);
    if !*removed {
        *removed = remove(path);
    }
    *removed
}

/// Whether `path` is gone afterwards.
fn remove(path: &Path) -> bool {
    match std::fs::remove_dir_all(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            tracing::warn!(path = %path.display(), err = %e, "failed to remove scratch");
            false
        }
    }
}

/// Remove the directory of every host that is no longer running.
fn sweep(root: &Path) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(root = %root.display(), err = %e, "cannot read the scratch root; sweeping nothing");
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let dir = entry.path();
        let is_host_dir = entry
            .file_name()
            .to_string_lossy()
            .starts_with(HOST_DIR_PREFIX)
            && entry.file_type().is_ok_and(|t| t.is_dir());
        if !is_host_dir {
            continue;
        }
        if owner_is_gone(&dir) {
            tracing::info!(dir = %dir.display(), "removing scratch left by a host that is no longer running");
            remove(&dir);
        }
    }
}

/// Whether the host that owns `dir` has exited: only when its lock can be
/// taken. Anything else, a missing lock file included, keeps the directory.
fn owner_is_gone(dir: &Path) -> bool {
    let Ok(lock) = File::open(dir.join(LOCK_FILE)) else {
        return false;
    };
    // Dropping `lock` releases it again before the directory is removed.
    lock.try_lock().is_ok()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn a_workload_dir_is_gone_once_released_or_dropped() {
        let root = tempfile::tempdir().unwrap();
        let scratch = ScratchRoot::open(root.path()).unwrap();

        let released = scratch.workload_dir().unwrap();
        std::fs::write(released.path().join("f"), "x").unwrap();
        let path = released.path().to_path_buf();
        released.remove();
        assert!(!path.exists());

        let dropped = scratch.workload_dir().unwrap();
        let path = dropped.path().to_path_buf();
        drop(dropped);
        assert!(!path.exists());
    }

    /// Once removed, a directory's name may go to another workload, so neither a
    /// second removal, such as a teardown retry, nor a later drop removes it again.
    #[test]
    fn a_removed_dir_never_deletes_what_later_takes_its_name() {
        let root = tempfile::tempdir().unwrap();
        let scratch = ScratchRoot::open(root.path()).unwrap();
        let released = scratch.workload_dir().unwrap();
        let stale = Arc::clone(&released);
        let path = released.path().to_path_buf();
        released.remove();
        drop(released);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("data"), "the next workload's").unwrap();
        stale.remove();
        assert!(
            path.join("data").exists(),
            "a second removal deleted a directory it no longer owns"
        );
        drop(stale);
        assert!(
            path.join("data").exists(),
            "a stale handle removed a directory it no longer owns"
        );
    }

    /// Only a success is remembered: a removal that failed is tried again.
    #[cfg(unix)]
    #[test]
    fn a_failed_removal_is_tried_again() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let scratch = ScratchRoot::open(root.path()).unwrap();
        let dir = scratch.workload_dir().unwrap();
        let path = dir.path().to_path_buf();
        std::fs::create_dir(path.join("sub")).unwrap();
        // Read-only, so `sub` cannot be unlinked from it.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
        let first = dir.remove();
        // Root removes it regardless, and there is nothing to test.
        if !path.exists() {
            return;
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!first, "a failed removal reported success");
        assert!(dir.remove(), "a failed removal was not tried again");
        assert!(!path.exists());
    }

    #[test]
    fn a_clean_shutdown_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let scratch = ScratchRoot::open(root.path()).unwrap();
        let workload = scratch.workload_dir().unwrap();
        drop(scratch);
        drop(workload);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    /// A crashed host never dropped its root: its directory is still there and
    /// its lock is free, so the next host removes it. A live host's is kept.
    #[test]
    fn the_sweep_removes_only_dead_hosts() {
        let root = tempfile::tempdir().unwrap();
        let live = ScratchRoot::open(root.path()).unwrap();
        let live_workload = live.workload_dir().unwrap();

        let crashed = root.path().join("host-crashed");
        std::fs::create_dir_all(crashed.join("workload-a")).unwrap();
        std::fs::write(crashed.join("workload-a/data"), "orphan").unwrap();
        File::create(crashed.join(LOCK_FILE)).unwrap();

        let next = ScratchRoot::open(root.path()).unwrap();
        assert!(!crashed.exists(), "a dead host's scratch is swept");
        assert!(
            live_workload.path().exists(),
            "a live host's scratch is kept"
        );
        assert!(next.dir.exists());
    }

    /// Ambiguity keeps: no lock file might be a host between creating its
    /// directory and locking it, and an unrelated directory is not ours.
    #[test]
    fn the_sweep_keeps_what_it_cannot_prove_dead() {
        let root = tempfile::tempdir().unwrap();
        let unlocked = root.path().join("host-starting");
        std::fs::create_dir_all(&unlocked).unwrap();
        let foreign = root.path().join("not-a-host");
        std::fs::create_dir_all(&foreign).unwrap();

        let _scratch = ScratchRoot::open(root.path()).unwrap();
        assert!(unlocked.exists());
        assert!(foreign.exists());
    }

    /// Hosts starting together on one root each sweep while the others are
    /// claiming: no sweep may take a lock or a directory from a starting host.
    #[test]
    fn hosts_starting_together_never_sweep_each_other() {
        const HOSTS: usize = 8;
        let root = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(HOSTS);
        let failures = std::sync::Mutex::new(Vec::new());
        // A failure is recorded, never panicked on: every thread must keep
        // reaching the barrier or the others wait forever.
        let record = |e: anyhow::Error| failures.lock().unwrap().push(format!("{e:#}"));
        std::thread::scope(|s| {
            for _ in 0..HOSTS {
                s.spawn(|| {
                    for _ in 0..50 {
                        barrier.wait();
                        let scratch = ScratchRoot::open(root.path());
                        // Every host has claimed before any workload starts.
                        barrier.wait();
                        match scratch {
                            Ok(scratch) => {
                                if let Err(e) = scratch.workload_dir() {
                                    record(e);
                                }
                            }
                            Err(e) => record(e),
                        }
                    }
                });
            }
        });
        let failures = failures.into_inner().unwrap();
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_root_sweeps_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let crashed = root.path().join("host-crashed");
        std::fs::create_dir_all(&crashed).unwrap();
        File::create(crashed.join(LOCK_FILE)).unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o300)).unwrap();
        // Root reads it regardless, and there is nothing to test.
        if std::fs::read_dir(root.path()).is_ok() {
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }
        sweep(root.path());
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(crashed.exists());
    }
}
