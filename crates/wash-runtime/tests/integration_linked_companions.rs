//! A linked component reached by sync calls runs in a companion store beside
//! its caller (see `engine::companion`). These drive the parts of that which a
//! single well-behaved call does not reach:
//!
//! ```text
//! caller --Token::new (sync)--> owner        (the owner gets a companion)
//! caller --adopter.ready (sync)--> adopter   (so does the adopter)
//! ```
//!
//! The owner counts its live tokens, which is how each test sees whether a
//! token it could no longer reach was dropped. It also reads files on request,
//! which is how the last test sees whose volumes each component has.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{
        Component, HostPathVolume, LocalResources, Volume, VolumeMount, VolumeType, Workload,
        WorkloadStartRequest,
    },
};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const CALLER_WASM: &[u8] = include_bytes!("wasm/companion_caller_p3.wasm");
const OWNER_WASM: &[u8] = include_bytes!("wasm/companion_owner_p3.wasm");
const ADOPTER_WASM: &[u8] = include_bytes!("wasm/companion_adopter_p3.wasm");

fn component(name: &str, bytes: &'static [u8], volume_mounts: Vec<VolumeMount>) -> Component {
    Component {
        name: name.to_string(),
        digest: None,
        bytes: bytes::Bytes::from_static(bytes),
        local_resources: LocalResources {
            volume_mounts,
            ..Default::default()
        },
        pool_size: 1,
        max_invocations: 100,
        max_concurrency: 1,
        ..Default::default()
    }
}

/// Start the three components as one workload and return the body the
/// caller's `route` answers with.
async fn run(route: &str) -> Result<String> {
    run_with_volumes(route, vec![], vec![], vec![]).await
}

/// [`run`], with `volumes` in the workload, mounted into the caller and the
/// owner as given.
async fn run_with_volumes(
    route: &str,
    volumes: Vec<Volume>,
    caller_mounts: Vec<VolumeMount>,
    owner_mounts: Vec<VolumeMount>,
) -> Result<String> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "linked-companions".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("companion-caller", CALLER_WASM, caller_mounts),
                component("companion-owner", OWNER_WASM, owner_mounts),
                component("companion-adopter", ADOPTER_WASM, vec![]),
            ],
            host_interfaces: http_only_host_interfaces("companions"),
            volumes,
        },
    })
    .await
    .context("workload should start")?;

    let response = timeout(
        Duration::from_secs(20),
        reqwest::Client::new()
            .get(format!("http://{addr}{route}"))
            .header("HOST", "companions")
            .send(),
    )
    .await
    .with_context(|| format!("{route} timed out"))?
    .with_context(|| format!("{route} failed"))?;
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(status.is_success(), "{route} answered {status}: {body:?}");
    Ok(body)
}

/// `wait` is sent before `signal` and cannot return until `signal` has run on
/// the same token, so this only completes if the companion serves both at once.
#[tokio::test]
async fn calls_on_one_companion_overlap() -> Result<()> {
    assert_eq!(run("/overlap").await?, "signalled");
    Ok(())
}

/// The caller abandons `make-held` once the owner has made its token, then
/// lets the owner finish. The token is returned to no one, so the owner has to
/// be told to drop it.
#[tokio::test]
async fn a_cancelled_calls_result_is_dropped_in_its_owner() -> Result<()> {
    assert_eq!(
        run("/cancel").await?,
        "started=true created=true dropped=true"
    );
    Ok(())
}

/// The adopter drops a token while the owner is inside a sync call from the
/// caller. A destructor cannot enter an instance mid-call, so the drop has to
/// run after that call returns, leaving the owner able to serve the next.
///
/// A companion takes no new work while a sync call runs, so today the drop
/// waits in its queue and this passes with or without the driver's own
/// deferral of staged drops.
#[tokio::test]
async fn a_drop_during_a_sync_call_runs_after_it() -> Result<()> {
    assert_eq!(
        run("/deferred-drop").await?,
        "ready=true pending=true dropped=true live=1"
    );
    Ok(())
}

/// A directory holding one file, `who`, that names whose volume it is.
fn volume_of(owner: &str) -> Result<(tempfile::TempDir, Volume)> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("who"), owner)?;
    let volume = Volume {
        name: owner.to_string(),
        volume_type: VolumeType::HostPath(HostPathVolume {
            local_path: dir.path().to_string_lossy().into_owned(),
        }),
    };
    Ok((dir, volume))
}

fn mount(volume: &str, mount_path: &str) -> VolumeMount {
    VolumeMount {
        name: volume.to_string(),
        mount_path: mount_path.to_string(),
        read_only: true,
    }
}

/// The caller and the owner each mount a volume of their own at `/data` and at
/// a path the other does not have. Each row is one reader's view of
/// `/data/who`, `/caller-only/who` and `/owner-only/who`: the caller's own,
/// then the owner's when called by a sync function (its companion store), a
/// plain async one (a store built for the call), and one carrying a stream
/// (the caller's store). The owner sees only what it mounted, however it is
/// reached, and the caller only what it did.
#[tokio::test]
async fn a_component_has_only_the_volumes_it_mounts() -> Result<()> {
    let (_caller_dir, caller_volume) = volume_of("caller")?;
    let (_owner_dir, owner_volume) = volume_of("owner")?;
    let seen = run_with_volumes(
        "/mounts",
        vec![caller_volume, owner_volume],
        vec![mount("caller", "/data"), mount("caller", "/caller-only")],
        vec![mount("owner", "/data"), mount("owner", "/owner-only")],
    )
    .await?;
    assert_eq!(
        seen.lines().collect::<Vec<_>>(),
        [
            "caller: caller caller -",
            "sync: owner - owner",
            "plain: owner - owner",
            "stream: owner - owner",
        ]
    );
    Ok(())
}
