//! Directory handles retained from volume validation through WASI preopens.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use wasmtime::error::Context as _;
use wasmtime_wasi::filesystem::{Descriptor, Dir};
use wasmtime_wasi::{FsPerms, OpenMode};

use crate::engine::ctx::ActiveCtx;
use crate::engine::workload::WorkloadComponent;
use crate::types::VolumeMount;

/// A directory opened without following links after its path was checked.
#[derive(Debug, Clone)]
pub(crate) struct OpenedVolume {
    pub(crate) path: PathBuf,
    dir: Arc<std::fs::File>,
}

/// A pinned directory with the permissions and name exposed to the guest.
#[derive(Clone)]
pub(crate) struct ResolvedVolumeMount {
    dir: Dir,
    mount_path: String,
}

impl ResolvedVolumeMount {
    pub(crate) fn from_opened(volume: &OpenedVolume, mount: &VolumeMount) -> anyhow::Result<Self> {
        let (perms, mode) = if mount.read_only {
            (FsPerms::ReadOnly, OpenMode::READ)
        } else {
            (FsPerms::ReadWrite, OpenMode::READ | OpenMode::WRITE)
        };
        Ok(Self {
            dir: Dir::new(volume.dir.try_clone()?, perms, mode, false),
            mount_path: mount.mount_path.clone(),
        })
    }

    pub(crate) async fn from_mount(host_path: &Path, mount: &VolumeMount) -> anyhow::Result<Self> {
        let path = host_path.to_path_buf();
        let volume =
            tokio::task::spawn_blocking(move || ReservedHostPaths::default().open(&path)).await??;
        Self::from_opened(&volume, mount)
    }
}

/// Expose retained handles alongside any preopens supplied by an embedder.
impl ActiveCtx<'_> {
    fn volume_directories(
        &mut self,
    ) -> wasmtime::Result<Vec<(wasmtime::component::Resource<Descriptor>, String)>> {
        let mut view = wasmtime_wasi::filesystem::WasiFilesystemCtxView {
            ctx: self.ctx.ctx.filesystem(),
            table: self.table,
        };
        let mut directories =
            wasmtime_wasi::p2::bindings::filesystem::preopens::Host::get_directories(&mut view)?;
        for mount in &self.ctx.volume_mounts {
            let descriptor = self.table.push(Descriptor::Dir(mount.dir.clone()))?;
            directories.push((descriptor, mount.mount_path.clone()));
        }
        Ok(directories)
    }
}

impl wasmtime_wasi::p2::bindings::filesystem::preopens::Host for ActiveCtx<'_> {
    fn get_directories(
        &mut self,
    ) -> wasmtime::Result<Vec<(wasmtime::component::Resource<Descriptor>, String)>> {
        self.volume_directories()
    }
}

impl wasmtime_wasi::p3::bindings::filesystem::preopens::Host for ActiveCtx<'_> {
    fn get_directories(
        &mut self,
    ) -> wasmtime::Result<Vec<(wasmtime::component::Resource<Descriptor>, String)>> {
        self.volume_directories()
    }
}

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
/// Open unresolved mounts outside the components lock. Existing handles are kept.
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

    /// Open the checked directory without following replacement symlinks.
    pub(crate) fn open(&self, volume: &Path) -> anyhow::Result<OpenedVolume> {
        let path = self.check(volume)?;
        let dir = open_canonical_dir(&path)
            .with_context(|| format!("failed to open hostPath volume '{}'", volume.display()))?;
        Ok(OpenedVolume {
            path,
            dir: Arc::new(dir),
        })
    }

    /// Check the path without exposing reserved names in the returned error.
    fn check(&self, volume: &std::path::Path) -> anyhow::Result<PathBuf> {
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

/// Walk from the root by handle, rejecting links at every component.
fn open_canonical_dir(path: &Path) -> std::io::Result<std::fs::File> {
    use cap_primitives::fs::{open_ambient_dir, open_dir_nofollow};
    use std::path::Component;

    let root: PathBuf = path
        .components()
        .take_while(|part| matches!(part, Component::Prefix(_) | Component::RootDir))
        .collect();
    let mut dir = open_ambient_dir(&root, cap_primitives::ambient_authority())?;
    for part in path.components().skip(root.components().count()) {
        if !matches!(part, Component::Normal(_)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "expected a canonical directory path",
            ));
        }
        dir = open_dir_nofollow(&dir, Path::new(part.as_os_str()))?;
    }
    Ok(dir)
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

#[cfg(all(test, unix))]
mod pinned_tests {
    use super::*;
    use crate::engine::ctx::Ctx;
    use std::os::unix::fs::symlink;
    use wasmtime::component::{Resource, ResourceTable};
    use wasmtime_wasi::p2::bindings::filesystem::types::{
        DescriptorFlags, HostDescriptor, OpenFlags, PathFlags,
    };

    #[test]
    fn a_link_swapped_after_the_check_is_refused_at_any_depth() {
        for replace_parent in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let safe = root.path().join("data/volume");
            let secret = root.path().join("secret/volume");
            std::fs::create_dir_all(&safe).unwrap();
            std::fs::create_dir_all(&secret).unwrap();
            let reserved = ReservedHostPaths::new([secret.clone()]);
            let checked = reserved.check(&safe).unwrap();
            let replaced = if replace_parent {
                safe.parent().unwrap()
            } else {
                &safe
            };
            std::fs::rename(replaced, root.path().join("original")).unwrap();
            let target = if replace_parent {
                "secret"
            } else {
                "../secret/volume"
            };
            symlink(target, replaced).unwrap();
            assert!(open_canonical_dir(&checked).is_err());
        }
    }

    #[tokio::test]
    async fn p2_and_p3_preopens_keep_the_checked_directory_and_permissions() {
        let root = tempfile::tempdir().unwrap();
        let safe = root.path().join("data");
        let secret = root.path().join("secret");
        std::fs::create_dir(&safe).unwrap();
        std::fs::create_dir(&secret).unwrap();
        std::fs::write(safe.join("value"), "safe").unwrap();
        std::fs::write(secret.join("value"), "private key").unwrap();
        let reserved = ReservedHostPaths::new([secret.join("value")]);
        let volume = reserved.open(&safe).unwrap();
        std::fs::rename(&safe, root.path().join("original")).unwrap();
        symlink("secret", &safe).unwrap();

        for p3 in [false, true] {
            for read_only in [false, true] {
                let mount = VolumeMount {
                    name: "data".into(),
                    mount_path: "/data".into(),
                    read_only,
                };
                let resolved = ResolvedVolumeMount::from_opened(&volume, &mount).unwrap();
                let mut ctx = Ctx::builder("workload", "component").build();
                ctx.volume_mounts.push(resolved);
                let mut table = ResourceTable::new();
                let mut active = ActiveCtx {
                    ctx: &mut ctx,
                    table: &mut table,
                };
                let mut directories = if p3 {
                    wasmtime_wasi::p3::bindings::filesystem::preopens::Host::get_directories(
                        &mut active,
                    )
                    .unwrap()
                } else {
                    wasmtime_wasi::p2::bindings::filesystem::preopens::Host::get_directories(
                        &mut active,
                    )
                    .unwrap()
                };
                assert_eq!(directories.len(), 1);
                let (directory, name) = directories.pop().unwrap();
                assert_eq!(name, "/data");
                let mut view = wasmtime_wasi::filesystem::WasiFilesystemCtxView {
                    ctx: ctx.ctx.filesystem(),
                    table: &mut table,
                };
                let file = view
                    .open_at(
                        Resource::new_borrow(directory.rep()),
                        PathFlags::empty(),
                        "value".into(),
                        OpenFlags::empty(),
                        DescriptorFlags::READ,
                    )
                    .await
                    .unwrap();
                assert_eq!(view.read(file, 32, 0).await.unwrap().0, b"safe");
                let write = view
                    .open_at(
                        Resource::new_borrow(directory.rep()),
                        PathFlags::empty(),
                        "new".into(),
                        OpenFlags::CREATE,
                        DescriptorFlags::WRITE,
                    )
                    .await;
                assert_eq!(write.is_err(), read_only);
            }
        }
        assert!(!secret.join("new").exists());
    }

    #[test]
    fn an_existing_allowed_symlink_can_be_pinned() {
        let root = tempfile::tempdir().unwrap();
        let safe = root.path().join("data");
        std::fs::create_dir(&safe).unwrap();
        symlink("data", root.path().join("alias")).unwrap();
        ReservedHostPaths::default()
            .open(&root.path().join("alias"))
            .unwrap();
    }
}
