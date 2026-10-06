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
//! token it could no longer reach was dropped.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const CALLER_WASM: &[u8] = include_bytes!("wasm/companion_caller_p3.wasm");
const OWNER_WASM: &[u8] = include_bytes!("wasm/companion_owner_p3.wasm");
const ADOPTER_WASM: &[u8] = include_bytes!("wasm/companion_adopter_p3.wasm");

fn component(name: &str, bytes: &'static [u8]) -> Component {
    Component {
        name: name.to_string(),
        digest: None,
        bytes: bytes::Bytes::from_static(bytes),
        local_resources: LocalResources::default(),
        pool_size: 1,
        max_invocations: 100,
        max_concurrency: 1,
        ..Default::default()
    }
}

/// Start the three components as one workload and return the body the
/// caller's `route` answers with.
async fn run(route: &str) -> Result<String> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "linked-companions".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("companion-caller", CALLER_WASM),
                component("companion-owner", OWNER_WASM),
                component("companion-adopter", ADOPTER_WASM),
            ],
            host_interfaces: http_only_host_interfaces("companions"),
            volumes: vec![],
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
