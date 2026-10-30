//! `emptyDir` volumes live under the host's scratch root and are removed when
//! their workload stops, and a restarted host removes what a crashed one left.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DynamicRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::types::{
    Component, EmptyDirVolume, LocalResources, Volume, VolumeMount, VolumeType, Workload,
    WorkloadStartRequest, WorkloadStopRequest,
};

mod common;
use common::http_only_host_interfaces;

const HTTP_SLEEPER_WASM: &[u8] = include_bytes!("wasm/http_sleeper.wasm");

/// Every file and directory under `dir` except host lock files.
fn scratch_entries(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(scratch_entries(&path));
            found.push(path);
        } else if !path.ends_with(".lock") {
            found.push(path);
        }
    }
    found
}

fn workload_with_scratch(name: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: name.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "sleeper".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_SLEEPER_WASM),
                local_resources: LocalResources {
                    volume_mounts: vec![VolumeMount {
                        name: "scratch".to_string(),
                        mount_path: "/scratch".to_string(),
                        read_only: false,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            }],
            host_interfaces: http_only_host_interfaces(name),
            volumes: vec![Volume {
                name: "scratch".to_string(),
                volume_type: VolumeType::EmptyDir(EmptyDirVolume {}),
            }],
        },
    }
}

#[tokio::test]
async fn stopped_workloads_leave_no_scratch() -> Result<()> {
    let root = tempfile::tempdir()?;
    let engine = Engine::builder().with_scratch_root(root.path()).build()?;
    let ingress = Ingress::new(DynamicRouter::default(), "127.0.0.1:0".parse()?).await?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .build()?
        .start()
        .await?;
    let idle = scratch_entries(root.path()).len();

    let mut ids = Vec::new();
    for n in 0..3 {
        let request = workload_with_scratch(&format!("scratch-{n}"));
        ids.push(request.workload_id.clone());
        let started = host.workload_start(request).await?;
        assert_eq!(
            started.workload_status.workload_state,
            wash_runtime::types::WorkloadState::Running,
            "{}",
            started.workload_status.message
        );
    }
    // Per workload: its directory and one volume directory.
    assert_eq!(scratch_entries(root.path()).len(), idle + 6);

    for workload_id in ids {
        host.workload_stop(WorkloadStopRequest { workload_id })
            .await?;
    }
    assert_eq!(
        scratch_entries(root.path()).len(),
        idle,
        "left behind: {:?}",
        scratch_entries(root.path())
    );
    Ok(())
}

/// A host that died without teardown left its directory and a free lock. The
/// next host on the same root removes it, and leaves a live host's alone.
#[tokio::test]
async fn a_restarted_host_sweeps_what_a_crashed_one_left() -> Result<()> {
    let root = tempfile::tempdir()?;
    let live = Engine::builder().with_scratch_root(root.path()).build()?;
    let live_workload = live.initialize_workload("live", workload_with_scratch("live").workload);
    assert!(live_workload.is_ok());

    let crashed = root.path().join("host-crashed");
    std::fs::create_dir_all(crashed.join("workload-x/volume-y"))?;
    std::fs::write(crashed.join("workload-x/volume-y/secret"), "orphaned")?;
    std::fs::File::create(crashed.join(".lock"))?;

    let _restarted = Engine::builder().with_scratch_root(root.path()).build()?;
    assert!(!crashed.exists(), "the crashed host's scratch is swept");
    assert!(
        scratch_entries(root.path()).iter().any(|p| p
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("volume-"))),
        "the live host's workload scratch survives"
    );
    Ok(())
}
