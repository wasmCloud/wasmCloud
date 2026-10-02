//! Where `emptyDir` volumes live, and how they are reclaimed.
//!
//! Scratch lives under one root the host owns, never loose in `$TMPDIR`: one
//! directory per host process, holding one directory per workload. That makes
//! the set on disk enumerable, which is what reclaiming it needs.
//!
//! Two mechanisms reclaim it, because neither covers every failure alone:
//!
//! - A workload's directory is removed when the workload is released, and
//!   again when its last handle drops, which covers a start that failed before
//!   it was bound.
//! - A host that crashed runs no teardown. Each host holds an exclusive lock on
//!   a file in its own directory for as long as it runs, and the kernel drops
//!   that lock when the process dies. When a host builds its engine it removes
//!   every other host directory whose lock it can take.
//!
//! The sweep deletes only what it can prove is dead. A directory whose lock
//! cannot be taken, whose lock file is missing, or which cannot be read is kept:
//! a directory lost to a sweep is another tenant's data, and one kept is disk.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        let lock = File::create(dir.join(LOCK_FILE))
            .with_context(|| format!("failed to create {}", dir.join(LOCK_FILE).display()))?;
        lock.try_lock()
            .map_err(|e| anyhow::anyhow!("failed to lock {}: {e}", dir.display()))?;
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
            _root: Arc::clone(self),
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
    /// Keeps the host directory, and its lock, alive while this lives.
    _root: Arc<ScratchRoot>,
}

impl ScratchDir {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the directory now, whoever else still holds it. An open handle
    /// keeps working on an unlinked directory on Unix; nothing new is written.
    pub(crate) fn remove(&self) {
        remove(&self.path);
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        remove(&self.path);
    }
}

fn remove(path: &Path) {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(path = %path.display(), err = %e, "failed to remove scratch"),
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
