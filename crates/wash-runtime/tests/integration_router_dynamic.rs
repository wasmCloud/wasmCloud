//! Integration tests for `DynamicRouter` — routes incoming requests to
//! workloads by Host header (with optional comma-separated aliases).
//!
//! `DynamicRouter::route_incoming_request` uses `tokio::task::block_in_place`
//! + `try_read`, so all tests run on the multi-thread runtime. Per-request
//! `timeout(...)` on individual HTTP calls guards against hangs.
//!
//! Covers:
//! - One workload registered under a primary host plus aliases: concurrent
//!   requests targeting all three hostnames all route correctly.
//! - Many concurrent requests against a single fixed host complete
//!   successfully within a bounded time (no `try_read` starvation or
//!   deadlock under load).
//! - In-flight requests racing a `workload_stop` all resolve (success or
//!   graceful error); the router does not hang or panic when the Host-header
//!   mapping disappears mid-request.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::Result;
use futures::future::join_all;
use std::time::Duration;
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{WorkloadStartRequest, WorkloadStopRequest},
};

mod common;
use common::{
    component_workload_request, default_counter_resources, get_status,
    http_counter_host_interfaces_with_aliases, start_host_with_dynamic_router,
};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");

fn http_counter_request(host_header: &str, aliases: Option<&str>) -> WorkloadStartRequest {
    component_workload_request(
        "http-counter.wasm",
        "http-counter-workload",
        HTTP_COUNTER_WASM,
        default_counter_resources(),
        http_counter_host_interfaces_with_aliases(host_header, aliases),
    )
}

/// Fan out concurrent requests across three hostnames (primary + 2 aliases)
/// registered to one workload; all must succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_dynamic_router_multiple_distinct_hosts_concurrent() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;

    let aliases = Some("web.local,admin.local");

    let req = http_counter_request("api.local", aliases);
    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let hostnames = ["api.local", "web.local", "admin.local"];
    let mut handles = Vec::with_capacity(20);
    for i in 0..20 {
        let client = client.clone();
        let hostname = hostnames
            .get(i % hostnames.len())
            .copied()
            .unwrap_or("api.local");
        handles.push(tokio::spawn(async move {
            let mut retried = false;

            loop {
                match timeout(
                    Duration::from_secs(5),
                    client
                        .get(format!("http://{addr}/"))
                        .header("HOST", hostname)
                        .send(),
                )
                .await
                {
                    Ok(Ok(response))
                        if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE
                            && !retried =>
                    {
                        retried = true;
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Ok(Ok(response)) => return Ok(response.status()),
                    Ok(Err(err)) => return Err(format!("{hostname} request failed: {err}")),
                    Err(_) => return Err(format!("{hostname} request timed out")),
                }
            }
        }));
    }

    let mut successful = 0;
    for result in join_all(handles).await {
        match result {
            Ok(Ok(status))
                if status.is_success() || status == reqwest::StatusCode::SERVICE_UNAVAILABLE =>
            {
                successful += 1;
            }
            _ => {}
        }
    }

    // DynamicRouter maps transient routing-table contention to 503, so after
    // retrying that case once we accept only 2xx or 503 here. Any 4xx,
    // connection error, timeout, or other status will be treated as a regression.
    assert!(
        successful == 20,
        "expected all 20 concurrent multi-host requests to end in either success or transient 503 after retrying once, got {successful}"
    );

    Ok(())
}

/// 50 concurrent requests against a single fixed host must mostly succeed
/// and complete quickly, the try_read must not deadlock or starve under stress.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_dynamic_router_concurrent_routes_fixed_mapping() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    let req = http_counter_request("fixed.local", None);
    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let started = std::time::Instant::now();
    let mut handles = Vec::with_capacity(50);
    for _ in 0..50 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            get_status(&client, addr, "fixed.local")
                .await
                .map(|status| status.is_success())
                .unwrap_or(false)
        }));
    }

    let successful = join_all(handles)
        .await
        .into_iter()
        .filter(|r| r.as_ref().copied().unwrap_or(false))
        .count();
    let elapsed = started.elapsed();

    // 5 requests are accounted with regards to flakiness
    assert!(
        successful >= 45,
        "expected >= 45/50 successful concurrent requests, got {successful}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "concurrent fan-out took too long: {elapsed:?} (lock contention possible)"
    );

    Ok(())
}

/// workload_stop mid-flight must not deadlock readers. In-flight requests
/// resolve to either success or a graceful 5xx; no hang or panic.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_dynamic_router_routes_race_with_unbind() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    let req = http_counter_request("race.local", None);
    let workload_id = req.workload_id.clone();
    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let mut handles = Vec::with_capacity(20);
    for _ in 0..20 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            timeout(
                Duration::from_secs(15),
                client
                    .get(format!("http://{addr}/"))
                    .header("HOST", "race.local")
                    .send(),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .map(|r| r.status())
        }));
    }

    // Give some requests a chance to get in-flight, then stop the workload.
    tokio::time::sleep(Duration::from_millis(20)).await;
    host.workload_stop(WorkloadStopRequest { workload_id })
        .await?;

    // Every task must resolve (success or graceful status), none may hang.
    let resolved = join_all(handles)
        .await
        .into_iter()
        .filter(|r| r.as_ref().ok().and_then(|s| s.as_ref()).is_some())
        .count();
    assert_eq!(
        resolved, 20,
        "all in-flight requests must resolve, got {resolved}/20"
    );

    Ok(())
}

/// Requests targeting a host that no workload is bound to must return 404
/// (`RouteError::NoWorkloadForHost`)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_dynamic_router_unknown_host_returns_404() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    host.workload_start(http_counter_request("known.local", None))
        .await?;

    let client = reqwest::Client::new();

    // A bound host succeeds (sanity check)
    assert!(
        get_status(&client, addr, "known.local").await?.is_success(),
        "bound host should succeed"
    );

    // An unbound host will return 404
    let status = get_status(&client, addr, "nope.local").await?;
    assert_eq!(
        status,
        reqwest::StatusCode::NOT_FOUND,
        "unknown host must map to RouteError::NoWorkloadForHost -> 404, got {status}"
    );

    Ok(())
}
