//! Volume-mount resolution for workload components.
//!
//! Component volume mounts arrive as `(host_path, VolumeMount)` pairs and must
//! be canonicalized (and turned into wasmtime preopen permissions) before a
//! store can preopen them. This module holds the resolved-mount value type
//! ([`ResolvedVolumeMount`]) plus the helpers that canonicalize a component's
//! mounts once and cache them on its [`WorkloadMetadata`], so request-path
//! store creation never re-canonicalizes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use wasmtime::error::Context as _;
use wasmtime_wasi::FsPerms;

use crate::engine::workload::WorkloadComponent;
use crate::types::VolumeMount;

/// A volume mount with its host path canonicalized and its
/// read-only/read-write flag turned into wasmtime preopen permissions.
///
/// Built once per component during workload resolution (see
/// [`resolve_component_volume_mounts_in_map`]) and reused by the store factory
/// when preopening directories, so the canonicalization cost stays off the
/// request path.
#[derive(Clone)]
pub(crate) struct ResolvedVolumeMount {
    pub(crate) host_path: PathBuf,
    pub(crate) mount_path: String,
    pub(crate) perms: FsPerms,
}

impl ResolvedVolumeMount {
    pub(crate) async fn from_mount(
        host_path: &PathBuf,
        mount: &VolumeMount,
    ) -> anyhow::Result<Self> {
        let host_path = tokio::fs::canonicalize(host_path)
            .await
            .with_context(|| format!("failed to canonicalize volume host path {host_path:?}"))?;
        let perms = match mount.read_only {
            true => FsPerms::ReadOnly,
            false => FsPerms::ReadWrite,
        };

        Ok(Self {
            host_path,
            mount_path: mount.mount_path.clone(),
            perms,
        })
    }
}

/// Canonicalize a list of `(host_path, VolumeMount)` pairs into
/// [`ResolvedVolumeMount`]s, preserving order.
pub(crate) async fn resolve_volume_mounts(
    volume_mounts: &[(PathBuf, VolumeMount)],
) -> anyhow::Result<Vec<ResolvedVolumeMount>> {
    let mut resolved = Vec::with_capacity(volume_mounts.len());
    for (host_path, mount) in volume_mounts {
        resolved.push(ResolvedVolumeMount::from_mount(host_path, mount).await?);
    }
    Ok(resolved)
}

/// Resolve and cache the volume mounts for the given components in the workload
/// component map.
///
/// For each component that has requested mounts but no resolved mounts yet, the
/// canonicalization runs without holding the components lock; the resolved
/// mounts are then written back under a single write lock. Components whose
/// mounts are already resolved are skipped, so this is cheap to call repeatedly.
pub(crate) async fn resolve_component_volume_mounts_in_map(
    components: &Arc<RwLock<BTreeMap<Arc<str>, WorkloadComponent>>>,
    component_ids: &[Arc<str>],
) -> anyhow::Result<()> {
    let pending = {
        let components = components.read().await;
        let mut pending = Vec::new();
        for component_id in component_ids {
            let component = components
                .get(component_id)
                .with_context(|| format!("component '{component_id}' not found"))?;
            if component.metadata.resolved_volume_mounts.is_empty()
                && !component.metadata.volume_mounts.is_empty()
            {
                pending.push((
                    component_id.clone(),
                    component.metadata.volume_mounts.clone(),
                ));
            }
        }
        pending
    };

    if pending.is_empty() {
        return Ok(());
    }

    let mut resolved = Vec::with_capacity(pending.len());
    for (component_id, volume_mounts) in pending {
        resolved.push((component_id, resolve_volume_mounts(&volume_mounts).await?));
    }

    let mut components = components.write().await;
    for (component_id, resolved_volume_mounts) in resolved {
        let component = components
            .get_mut(&component_id)
            .with_context(|| format!("component '{component_id}' not found"))?;
        if component.metadata.resolved_volume_mounts.is_empty() {
            component.metadata.resolved_volume_mounts = resolved_volume_mounts;
        }
    }

    Ok(())
}

/// Host paths no workload volume may expose: the host's own credentials and
/// configuration.
///
/// A `hostPath` volume is refused when it lies inside a reserved path, or
/// contains one, since a volume containing a credential file exposes it — and,
/// when writable, lets the workload replace it.
///
/// Each reserved path is resolved again at every check, not once: a Kubernetes
/// Secret file resolves through a `..data` link that rotation repoints, and a
/// path such as an OCI cache may not exist until after the host starts. It is
/// matched both as written and as it resolves now, and a path that does not
/// exist resolves through its nearest existing ancestor.
#[derive(Debug, Clone, Default)]
pub struct ReservedHostPaths(Arc<[PathBuf]>);

impl ReservedHostPaths {
    /// Reserve `paths`; a relative one is taken against the working directory.
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        let mut reserved: Vec<PathBuf> = paths
            .into_iter()
            .map(|path| {
                if path.is_relative() {
                    cwd.join(path)
                } else {
                    path
                }
            })
            .collect();
        reserved.sort();
        reserved.dedup();
        Self(reserved.into())
    }

    /// Check a `hostPath` volume at `volume`, returning the canonical path it
    /// was checked as — the one to mount, so the path mounted is the path
    /// checked. The error does not name the reserved path, since it reaches
    /// whoever deployed the workload; the host logs it.
    pub fn check(&self, volume: &std::path::Path) -> anyhow::Result<PathBuf> {
        let canonical = volume.canonicalize().map_err(|err| {
            anyhow::anyhow!(
                "failed to resolve hostPath volume '{}': {err}",
                volume.display()
            )
        })?;
        let exposed = |reserved: &std::path::Path| {
            canonical.starts_with(reserved) || reserved.starts_with(&canonical)
        };
        match self
            .0
            .iter()
            .find(|reserved| exposed(reserved) || exposed(&resolve_existing(reserved)))
        {
            Some(reserved) => {
                tracing::warn!(
                    volume = %canonical.display(),
                    reserved = %reserved.display(),
                    "refused a hostPath volume that would expose a reserved host path"
                );
                anyhow::bail!(
                    "hostPath volume '{}' would expose a path this host reserves for its own \
                     credentials or configuration",
                    volume.display()
                )
            }
            None => Ok(canonical),
        }
    }
}

/// `path` canonicalized through its nearest existing ancestor, with the part
/// that does not exist yet joined back on.
fn resolve_existing(path: &std::path::Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = current.canonicalize() {
            return missing
                .iter()
                .rev()
                .fold(canonical, |acc, part| acc.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod reserved_tests {
    use super::*;

    #[test]
    fn a_volume_inside_or_around_a_reserved_path_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let creds = root.path().join("creds");
        let data = root.path().join("data");
        std::fs::create_dir_all(creds.join("nested")).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(creds.join("client.key"), "k").unwrap();
        let reserved = ReservedHostPaths::new([creds.join("client.key")]);

        // Containing the file, at any depth, exposes it.
        assert!(reserved.check(&creds).is_err());
        assert!(reserved.check(root.path()).is_err());
        // A sibling directory does not.
        assert_eq!(reserved.check(&data).unwrap(), data.canonicalize().unwrap());
        reserved.check(&creds.join("nested")).unwrap();

        let reserved = ReservedHostPaths::new([creds.clone()]);
        assert!(reserved.check(&creds.join("nested")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_does_not_step_around_a_reservation() {
        let root = tempfile::tempdir().unwrap();
        let creds = root.path().join("creds");
        std::fs::create_dir_all(&creds).unwrap();
        let link = root.path().join("innocent");
        std::os::unix::fs::symlink(&creds, &link).unwrap();
        let reserved = ReservedHostPaths::new([creds]);
        assert!(reserved.check(&link).is_err());
    }

    /// A Kubernetes Secret volume: `tls.key` is a link through `..data`, which
    /// rotation repoints at a new timestamped directory.
    #[cfg(unix)]
    #[test]
    fn a_rotated_secret_stays_reserved() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let secret = root.path().join("secret");
        std::fs::create_dir_all(secret.join("..2026_1")).unwrap();
        std::fs::write(secret.join("..2026_1/tls.key"), "k1").unwrap();
        symlink("..2026_1", secret.join("..data")).unwrap();
        symlink("..data/tls.key", secret.join("tls.key")).unwrap();
        let reserved = ReservedHostPaths::new([secret.join("tls.key")]);

        // Rotation: a new directory, and `..data` swapped onto it.
        std::fs::create_dir_all(secret.join("..2026_2")).unwrap();
        std::fs::write(secret.join("..2026_2/tls.key"), "k2").unwrap();
        std::fs::remove_file(secret.join("..data")).unwrap();
        symlink("..2026_2", secret.join("..data")).unwrap();

        assert!(reserved.check(&secret.join("..data")).is_err());
        assert!(reserved.check(&secret.join("..2026_2")).is_err());
        assert!(reserved.check(&secret).is_err());
    }

    /// A reserved path created after the host started, such as an OCI cache
    /// on its first pull, is reserved from then on.
    #[test]
    fn a_path_created_later_is_reserved_once_it_exists() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        let reserved = ReservedHostPaths::new([cache.clone()]);
        std::fs::create_dir_all(cache.join("blobs")).unwrap();
        assert!(reserved.check(&cache).is_err());
        assert!(reserved.check(&cache.join("blobs")).is_err());
        assert!(reserved.check(root.path()).is_err());
    }
}
