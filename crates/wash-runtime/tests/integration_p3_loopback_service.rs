//! Regression test for p3 guest-spawned task scheduling: a tick-free
//! `cli/run` service serving the virtualized loopback must answer promptly.
//!
//! Historically, a long-lived p3 service's spawned tasks were only polled
//! when the executor was re-entered by a host event, so sequential
//! guest-to-guest loopback requests lagged by one and services carried a
//! periodic clock-tick workaround. This test pins the fixed behavior with a
//! deliberately tick-free service: `svc-tcp-echo` accepts loopback
//! connections in `cli/run` and drives every reply through spawned tasks,
//! while the `http-loopback-gateway` component proxies one HTTP request at a
//! time into it. If spawned tasks ever again require an unrelated host event
//! to make progress, the first request here hangs (each request is awaited to
//! completion before the next is sent, so nothing else re-enters the store)
//! and the per-request timeout fails the test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, time::Duration, time::Instant};

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const SVC_TCP_ECHO_WASM: &[u8] = include_bytes!("wasm/svc_tcp_echo.wasm");
const HTTP_LOOPBACK_GATEWAY_WASM: &[u8] = include_bytes!("wasm/http_loopback_gateway.wasm");

fn echo_workload(host: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(SVC_TCP_ECHO_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            components: vec![Component {
                name: "http-loopback-gateway".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_LOOPBACK_GATEWAY_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 1000,
            }],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

/// Sequential loopback round-trips complete promptly without any tick in the
/// service. Requests are strictly serialized (each awaited before the next),
/// so a service whose spawned tasks need an unrelated host event to progress
/// has nothing to re-enter it — the request would hang until the timeout.
#[tokio::test]
async fn test_tickless_loopback_service_answers_promptly() -> Result<()> {
    let host_name = "loopback-echo";
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    host.workload_start(echo_workload(host_name))
        .await
        .context("failed to start loopback echo workload")?;

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;

    for i in 0..5 {
        let started = Instant::now();
        let resp = timeout(
            Duration::from_secs(5),
            client
                .get(format!("http://{addr}/"))
                .header("HOST", host_name)
                .send(),
        )
        .await
        .with_context(|| {
            format!(
                "request {i} did not complete: the echo service's spawned tasks \
                 were not driven without an unrelated host event"
            )
        })??;
        anyhow::ensure!(
            resp.status().is_success(),
            "request {i} failed: {}",
            resp.status()
        );
        let body = resp.text().await?;
        anyhow::ensure!(
            body == "echo:ping",
            "request {i} returned unexpected body {body:?}"
        );
        // Well under the timeout: catch "slow but not hung" regressions too.
        let elapsed = started.elapsed();
        anyhow::ensure!(
            elapsed < Duration::from_secs(2),
            "request {i} took {elapsed:?}; loopback round-trips should be fast"
        );
    }

    Ok(())
}
