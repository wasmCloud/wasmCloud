//! Integration tests for `DevRouter` — the development-mode router that
//! dispatches every request to the last resolved workload.
//!
//! Covers:
//! - `last_workload_id` is updated on each `on_workload_resolved`, proved by
//!   distinguishable response bodies from two simultaneously-registered
//!   components (http-counter vs. http-handler-p2).
//! - `on_workload_unbind` clears `last_workload_id` only when the stopped id
//!   matches the currently-bound workload; stopping any other workload is a
//!   no-op for routing.
//! - Concurrent reads racing a workload swap all resolve to a response (no
//!   hangs, no panics from `try_lock` contention).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::Result;
use futures::future::join_all;
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{LocalResources, WorkloadStartRequest, WorkloadStopRequest},
};

mod common;
use common::{
    component_workload_request, default_counter_resources, get_status, get_status_and_body,
    http_counter_host_interfaces, http_only_host_interfaces, start_host_with_dev_router,
};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");
const HTTP_HANDLER_P2_WASM: &[u8] = include_bytes!("wasm/http_handler_p2.wasm");

fn http_counter_request(host_header: &str) -> WorkloadStartRequest {
    component_workload_request(
        "http-counter.wasm",
        "http-counter-workload",
        HTTP_COUNTER_WASM,
        default_counter_resources(),
        http_counter_host_interfaces(host_header),
    )
}

fn http_handler_p2_request(host_header: &str) -> WorkloadStartRequest {
    component_workload_request(
        "http-handler-p2.wasm",
        "http-handler-p2-workload",
        HTTP_HANDLER_P2_WASM,
        LocalResources {
            memory_limit_mb: 128,
            cpu_limit: 1,
            config: HashMap::new(),
            environment: HashMap::new(),
            volume_mounts: vec![],
            allowed_hosts: Default::default(),
        },
        http_only_host_interfaces(host_header),
    )
}

/// `DevRouter` with no bound workload reports `RouteError::NoWorkloadForHost("")`,
/// which `handle_http_request` maps to 404.
fn is_unrouted(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::NOT_FOUND
}

/// With A and B both registered, DevRouter routes to last-resolved B;
/// stopping B clears routing (with no fallback to A).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_dev_router_last_workload_wins() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (addr, host) = start_host_with_dev_router("127.0.0.1:0").await?;
    let client = reqwest::Client::new();

    // Only "A" registered. http-counter hits example.com; body is a digit.
    let req_a = http_counter_request("a");
    let id_a = req_a.workload_id.clone();
    host.workload_start(req_a).await?;

    let (status_a, body_a) = get_status_and_body(&client, addr, "anything").await?;
    assert!(
        status_a.is_success(),
        "with only A registered, request should succeed, got {status_a}"
    );
    assert!(
        body_a
            .trim()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit()),
        "http-counter response body should start with a digit, got {body_a:?}"
    );

    // Start "B", Now both are registered, but B is the last-resolved.
    let req_b = http_handler_p2_request("b");
    let id_b = req_b.workload_id.clone();
    host.workload_start(req_b).await?;

    let (status_b, body_b) = get_status_and_body(&client, addr, "anything").await?;
    assert!(
        status_b.is_success(),
        "with A+B registered, request should succeed via B, got {status_b}"
    );
    assert_eq!(
        body_b.trim(),
        "hello from p2",
        "DevRouter should dispatch to last-resolved B (http-handler-p2), \
         not A (http-counter); got body {body_b:?}"
    );

    // Stop B. DevRouter's only target is cleared so it does not fall back
    // to A (which is still registered). Requests will fail.
    host.workload_stop(WorkloadStopRequest { workload_id: id_b })
        .await?;
    let status = get_status(&client, addr, "anything").await?;
    assert!(
        is_unrouted(status),
        "after stopping last-resolved B, request must not route (A remains \
         registered but is not the DevRouter target), got {status}"
    );

    // Clean up A.
    host.workload_stop(WorkloadStopRequest { workload_id: id_a })
        .await?;

    Ok(())
}

/// `on_workload_unbind` clears `last_workload_id` only if the stopped id
/// matches the currently-bound one. Stopping a non-current workload must not
/// affect routing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_dev_router_unbind_clears_only_matching() -> Result<()> {
    let (addr, host) = start_host_with_dev_router("127.0.0.1:0").await?;
    let client = reqwest::Client::new();

    let req_a = http_counter_request("a");
    let id_a = req_a.workload_id.clone();
    host.workload_start(req_a).await?;

    // Starting B makes B the "last" workload. A is still registered but
    // is no longer the routing target.
    let req_b = http_counter_request("b");
    let id_b = req_b.workload_id.clone();
    host.workload_start(req_b).await?;

    // Stopping A (non-current one) should not change the DevRouter mapping;
    // requests still route (to B).
    host.workload_stop(WorkloadStopRequest { workload_id: id_a })
        .await?;
    assert!(
        get_status(&client, addr, "whatever").await?.is_success(),
        "stopping non-current workload must not unbind router"
    );

    // Stopping B clears the binding so subsequent requests are rejected by
    // the router (404 Not Found via RouteError::NoWorkloadForHost).
    host.workload_stop(WorkloadStopRequest { workload_id: id_b })
        .await?;
    let status = get_status(&client, addr, "whatever").await?;
    assert!(
        is_unrouted(status),
        "after stopping current workload, request should not succeed, got {status}"
    );

    Ok(())
}

/// Concurrent reads during a workload swap must all resolve (success or
/// graceful 5xx). Exercises `try_lock()` contention on `last_workload_id`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_dev_router_concurrent_reads_during_swap() -> Result<()> {
    let (addr, host) = start_host_with_dev_router("127.0.0.1:0").await?;

    let req_a = http_counter_request("a");
    let id_a = req_a.workload_id.clone();
    host.workload_start(req_a).await?;

    // Spawn 30 readers that each sleep a small staggered amount & then run.
    let client = reqwest::Client::new();
    let mut reader_handles = Vec::with_capacity(30);
    for i in 0..30 {
        let client = client.clone();
        reader_handles.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis((i as u64) * 5)).await;
            timeout(
                Duration::from_secs(15),
                client
                    .get(format!("http://{addr}/"))
                    .header("HOST", "whatever")
                    .send(),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .map(|r| r.status())
        }));
    }

    // Midway through the staggered window, swap A -> B on the main task.
    tokio::time::sleep(Duration::from_millis(40)).await;
    host.workload_stop(WorkloadStopRequest { workload_id: id_a })
        .await?;
    let req_b = http_counter_request("b");
    host.workload_start(req_b).await?;

    let resolved = join_all(reader_handles)
        .await
        .into_iter()
        .filter(|r| r.as_ref().ok().and_then(|s| s.as_ref()).is_some())
        .count();
    assert_eq!(
        resolved, 30,
        "every reader must resolve (no hangs); got {resolved}/30"
    );

    Ok(())
}
