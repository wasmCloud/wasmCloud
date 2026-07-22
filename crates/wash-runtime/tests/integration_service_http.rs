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
use common::{
    http_only_host_interfaces, json_u64_field, start_host_with_dynamic_router,
    start_host_with_p3_http_handler,
};

const SVC_COUNTER_WASM: &[u8] = include_bytes!("wasm/svc_counter.wasm");

/// Parse `{"cli_ticks":N,"http_calls":M}` without pulling in a JSON dep.
fn parse_counter(body: &str) -> (u64, u64) {
    (
        json_u64_field(body, "cli_ticks"),
        json_u64_field(body, "http_calls"),
    )
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
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    host.workload_start(svc_counter_request("svc-counter"))
        .await
        .context("failed to start p3 service-http workload")?;

    // No connection pooling: a GET retried on a stale pooled connection would
    // land twice on the instance and break the exactly-once http_calls counts.
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;

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

/// Regression for "N service replicas on one host serve like one". Two defects
/// combined to pin all traffic to a single replica: service workloads never
/// registered their hostname with the `DynamicRouter` (so a hostname router
/// 404'd them), and `route_incoming_request` picked one arbitrary replica per
/// host. With both fixed, four `svc-counter` replicas bound to ONE hostname
/// share the traffic.
///
/// Selection is random, so this asserts every replica gets used rather than an
/// exact split. Each replica keeps an independent `http_calls` counter that
/// starts at 0, so the first request routed to a given replica is the only one
/// that reads back `http_calls == 1`. Counting the `== 1` responses therefore
/// counts the distinct replicas that served at least once — which must be all
/// four. The old pin-to-one behavior sends every request to one replica, so
/// exactly one response reads `1` (and the rest climb `2,3,4,…`), failing this.
///
/// Runs on a multi-thread runtime so the HTTP server and the four co-driven p3
/// service instances make progress in parallel with the request loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_service_http_spreads_load_across_replicas() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;

    const REPLICAS: usize = 4;
    for _ in 0..REPLICAS {
        host.workload_start(svc_counter_request("svc-rr"))
            .await
            .context("failed to start svc-counter replica")?;
    }

    // No connection pooling: a GET retried on a stale pooled connection would
    // land twice on one instance and skew the per-replica counts.
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;

    // Enough requests that random selection hits all four replicas with
    // overwhelming probability: P(any replica missed) <= 4 * (3/4)^80 ~= 4e-10.
    const REQUESTS: usize = 80;
    let mut distinct_replicas = 0;
    for _ in 0..REQUESTS {
        let resp = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}/"))
                .header("HOST", "svc-rr")
                .send(),
        )
        .await??;
        anyhow::ensure!(
            resp.status().is_success(),
            "each replica should serve requests, got {}",
            resp.status()
        );
        let (_cli_ticks, calls) = parse_counter(&resp.text().await?);
        if calls == 1 {
            distinct_replicas += 1;
        }
    }

    assert_eq!(
        distinct_replicas, REPLICAS,
        "all {REPLICAS} replicas should serve traffic (one `http_calls == 1` each); \
         saw {distinct_replicas} — a single pinned replica would show exactly 1"
    );

    Ok(())
}
