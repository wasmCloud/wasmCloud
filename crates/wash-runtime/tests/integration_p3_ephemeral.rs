//! Integration test for the WASIP3 ephemeral-store linked-call path.
//!
//! `ephemeral-caller-p3` (a P3 HTTP handler) imports a plain-value async
//! function `run(u32) -> u32` from a second P3 component
//! (`ephemeral-callee-p3`) and returns its result as the response body.
//!
//! Because the linked function's signature is *all plain values* (no
//! resource/stream/future/borrow handles), the dynamic linker dispatches the
//! call through the ephemeral-store path
//! (`ResolvedWorkload::resolve_component_imports` →
//! `invoke_ephemeral_linked_export`): the callee is instantiated in a
//! short-lived `Store` that is dropped — and its core-instance slots reclaimed —
//! as soon as the call returns. The streaming/resource fixtures only exercise
//! the shared-store path, so this is the only coverage of the ephemeral path.
//!
//! A correct round-trip body (`run(21) == 43`) proves the call crossed the
//! linker into a fresh ephemeral store, executed, and returned its plain value
//! back to the caller.
//! Run with `RUST_LOG=wash_runtime::engine::workload=trace` to see the
//! `invoked ephemeral dynamic export` trace for each request.

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

const EPHEMERAL_CALLER_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_caller_p3.wasm");
const EPHEMERAL_CALLEE_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_callee_p3.wasm");

#[tokio::test]
async fn test_p3_plain_value_async_call_uses_ephemeral_store() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-ephemeral".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "ephemeral-caller".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(EPHEMERAL_CALLER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: "ephemeral-callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(EPHEMERAL_CALLEE_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
            ],
            host_interfaces: http_only_host_interfaces("p3-ephemeral"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("ephemeral cross-component workload should start")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-ephemeral")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    assert!(
        response.status().is_success(),
        "ephemeral handler should return 2xx, got {}",
        response.status()
    );

    let body = response.text().await?;
    assert_eq!(
        body, "43",
        "ephemeral cross-component call should return run(21) == 43"
    );

    let response2 = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-ephemeral")
            .send(),
    )
    .await
    .context("second request timed out")?
    .context("second request failed")?;
    assert!(response2.status().is_success());
    assert_eq!(response2.text().await?, "43");

    Ok(())
}
