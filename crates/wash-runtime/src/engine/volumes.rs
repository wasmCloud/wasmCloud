//! Volume-mount resolution for workload components.
//!
//! Component volume mounts arrive as `(host_path, VolumeMount)` pairs and must
//! be canonicalized (and turned into wasmtime preopen permissions) before a
//! store can preopen them. This module holds the resolved-mount value type
//! ([`ResolvedVolumeMount`]) plus the helpers that canonicalize a component's
//! mounts once and cache them on its [`WorkloadMetadata`], so request-path
//! store creation never re-canonicalizes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use wasmtime::error::Context as _;
use wasmtime_wasi::{DirPerms, FilePerms};

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
    pub(crate) dir_perms: DirPerms,
    pub(crate) file_perms: FilePerms,
}

impl ResolvedVolumeMount {
    pub(crate) async fn from_mount(
        host_path: &PathBuf,
        mount: &VolumeMount,
    ) -> anyhow::Result<Self> {
        let host_path = tokio::fs::canonicalize(host_path)
            .await
            .with_context(|| format!("failed to canonicalize volume host path {host_path:?}"))?;
        let (dir_perms, file_perms) = match mount.read_only {
            true => (DirPerms::READ, FilePerms::READ),
            false => (DirPerms::all(), FilePerms::all()),
        };

        Ok(Self {
            host_path,
            mount_path: mount.mount_path.clone(),
            dir_perms,
            file_perms,
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
    components: &Arc<RwLock<HashMap<Arc<str>, WorkloadComponent>>>,
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
