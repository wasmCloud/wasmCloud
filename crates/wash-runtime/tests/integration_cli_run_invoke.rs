//! Invoking `wasi:cli/run` on a workload *component*.
//!
//! `wasi:cli/run` names a component's entrypoint. Beside the host calling it on
//! a workload's service, a component of the workload can export it and
//! something else decide when to run it.
//!
//! The caller here is the workload's trigger service (`cli-run-caller`), which
//! imports `wasi:cli/run` and dispatches one run per inbound HTTP request.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_p3_http_handler};

const CLI_RUN_CALLER_WASM: &[u8] = include_bytes!("wasm/cli_run_caller.wasm");
const CLI_RUN_CALLEE_WASM: &[u8] = include_bytes!("wasm/cli_run_callee.wasm");

/// A workload whose trigger service imports `wasi:cli/run` and whose single
/// component exports it, ending each run the way `mode` says.
fn cli_run_request_with_mode(host: &str, mode: &str) -> WorkloadStartRequest {
    let mut req = cli_run_request(host);
    for component in &mut req.workload.components {
        component
            .local_resources
            .environment
            .insert("RUN_MODE".to_string(), mode.to_string());
    }
    req
}

/// A workload whose trigger service imports `wasi:cli/run` and whose single
/// component exports it.
fn cli_run_request(host: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(CLI_RUN_CALLER_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            components: vec![Component {
                name: "cli-run-callee".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(CLI_RUN_CALLEE_WASM),
                local_resources: LocalResources::default(),
                pool_size: 0,
                max_invocations: 100,
                max_concurrency: 1,
            }],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

/// The service's imported `wasi:cli/run` reaches the component exporting it,
/// once per request.
#[tokio::test]
async fn test_service_invokes_component_cli_run() -> Result<()> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    host.workload_start(cli_run_request("cli-run"))
        .await
        .context("failed to start the cli/run workload")?;

    // No connection pooling: a GET retried on a stale pooled connection would
    // dispatch twice and break the exactly-once dispatch counts.
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;

    let dispatch = || async {
        let resp = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}/run"))
                .header("HOST", "cli-run")
                .send(),
        )
        .await??;
        anyhow::ensure!(
            resp.status().is_success(),
            "service should dispatch the run, got {}",
            resp.status()
        );
        let body = resp.text().await?;
        anyhow::ensure!(
            body.contains("\"ok\":true"),
            "the component's run should succeed, got {body}"
        );
        Ok::<_, anyhow::Error>(json_u64_field(&body, "dispatched"))
    };

    assert_eq!(dispatch().await?, 1, "first request dispatches one run");
    assert_eq!(dispatch().await?, 2, "second request dispatches another");

    Ok(())
}

/// Every way a run can end reaches the caller as a value. `wasi:cli/exit`
/// unwinds the callee rather than returning, and an error out of a linked call
/// faults the caller's store — so without containment a callee exiting, even
/// successfully, would take down the service that ran it.
#[tokio::test]
async fn test_run_endings_reach_the_caller() -> Result<()> {
    for (mode, expected) in [
        ("ok", true),
        ("err", false),
        ("exit-ok", true),
        ("exit-err", false),
    ] {
        let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
        host.workload_start(cli_run_request_with_mode("cli-run", mode))
            .await?;
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .build()?;
        let resp = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}/run"))
                .header("HOST", "cli-run")
                .send(),
        )
        .await??;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "the service should survive a {mode} run and answer"
        );
        let body = resp.text().await?;
        assert!(
            body.contains(&format!("\"ok\":{expected}")),
            "a {mode} run should read as ok={expected}, got {body}"
        );
    }
    Ok(())
}
