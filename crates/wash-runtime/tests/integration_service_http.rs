//! Integration tests for the p3 service HTTP ingress co-driver.
//!
//! A p3 service that exports BOTH `wasi:cli/run@0.3` and
//! `wasi:http/handler@0.3` is co-driven on a single instance: the host keeps
//! `cli/run` running while delivering inbound HTTP to the same instance's
//! `http/handler`. The `svc-counter` fixture proves this by incrementing a
//! process-global counter from its `cli/run` loop and reporting it from each
//! HTTP response — a response observing `cli_ticks > 0` that grows across
//! requests can only happen if the run loop is co-driven concurrently.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::types::{LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3};

const SVC_COUNTER_WASM: &[u8] = include_bytes!("wasm/svc_counter.wasm");

/// Parse `{"cli_ticks":N,"http_calls":M}` without pulling in a JSON dep.
fn parse_counter(body: &str) -> (u64, u64) {
    let field = |name: &str| -> u64 {
        let key = format!("\"{name}\":");
        let start = body.find(&key).expect("field present") + key.len();
        let rest = &body[start..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..end].parse().expect("numeric field")
    };
    (field("cli_ticks"), field("http_calls"))
}

fn svc_counter_request(host: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(SVC_COUNTER_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

/// The service co-drives `cli/run` (ticking a counter) while serving HTTP on the
/// same instance: the HTTP response sees a non-zero, growing `cli_ticks`.
#[tokio::test]
async fn test_service_http_co_drives_cli_run() -> Result<()> {
    let (addr, host) = start_host_with_p3("127.0.0.1:0").await?;

    host.workload_start(svc_counter_request("svc-counter"))
        .await
        .context("failed to start p3 service-http workload")?;

    let client = reqwest::Client::new();

    let get = || async {
        let resp = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}/"))
                .header("HOST", "svc-counter")
                .send(),
        )
        .await??;
        anyhow::ensure!(
            resp.status().is_success(),
            "service should serve requests, got {}",
            resp.status()
        );
        let (cli_ticks, http_calls) = parse_counter(&resp.text().await?);
        Ok::<_, anyhow::Error>((cli_ticks, http_calls))
    };

    // Request once to confirm the service instance serves HTTP at all. The very
    // first request can race the 10ms run-loop tick, so cli_ticks may still be 0
    // here — that's expected.
    let (_ticks1, calls1) = get().await?;
    assert_eq!(calls1, 1, "first request is http_calls=1");

    // Poll over a window: cli/run is co-driven on the same instance, so its tick
    // counter must climb past zero while we keep serving HTTP. Each response also
    // increments http_calls on the SAME instance (shared in-memory state, not a
    // fresh per-request instance).
    let mut last_ticks = 0;
    let mut last_calls = calls1;
    let mut saw_growth = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (ticks, calls) = get().await?;
        assert_eq!(
            calls,
            last_calls + 1,
            "each request lands on the same long-lived instance (expected http_calls={}, got {calls})",
            last_calls + 1
        );
        last_calls = calls;
        if ticks > last_ticks && last_ticks > 0 {
            // cli/run advanced between two HTTP requests → co-driven concurrently.
            saw_growth = true;
            break;
        }
        if ticks > 0 {
            last_ticks = ticks;
        }
    }

    assert!(
        saw_growth,
        "cli/run should be co-driven concurrently with HTTP serving — cli_ticks never grew between requests"
    );

    Ok(())
}
