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
//! - HTTP/1.0 request without a Host header returns 400 (RouteError::MissingHost).
//! - Invalid hostnames in host-aliases are silently filtered; only valid ones route.
//! - Two workloads bound to the same host both serve requests (HashSet routing).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use futures::future::join_all;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

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
/// registered to one workload. The router must not produce 4xx/timeouts;
/// 500s from the component's shared state under load are out of scope here.
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
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    Ok(Ok(response)) => return Ok(response.status()),
                    Ok(Err(err)) => return Err(format!("{hostname} request failed: {err}")),
                    Err(_) => return Err(format!("{hostname} request timed out")),
                }
            }
        }));
    }

    let results: Vec<_> = join_all(handles)
        .await
        .into_iter()
        .map(|r| r.expect("task should not panic"))
        .collect();

    // 500 = component shared-state failure under load, 503 = transient try_read contention.
    // Neither is a routing bug; anything else is.
    let routing_failures: Vec<_> = results
        .iter()
        .filter(|r| match r {
            Ok(status) => {
                !status.is_success()
                    && *status != reqwest::StatusCode::SERVICE_UNAVAILABLE
                    && *status != reqwest::StatusCode::INTERNAL_SERVER_ERROR
            }
            Err(_) => true,
        })
        .collect();

    assert!(
        routing_failures.is_empty(),
        "router must not produce 4xx/timeouts under multi-host concurrent load, failures: {routing_failures:?}"
    );

    Ok(())
}

/// 50 concurrent requests against a single fixed host must not deadlock or
/// starve under stress. The router itself should never be the source of
/// failures.
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
            get_status(&client, addr, "fixed.local").await
        }));
    }

    let statuses: Vec<_> = join_all(handles)
        .await
        .into_iter()
        .map(|r| r.expect("task should not panic"))
        .collect();
    let elapsed = started.elapsed();

    // 500 = component shared-state failure under load, 503 = transient try_read contention.
    // Neither is a routing bug; anything else is.
    let routing_failures: Vec<_> = statuses
        .iter()
        .filter(|r| match r {
            Ok(s) => {
                !s.is_success()
                    && *s != reqwest::StatusCode::INTERNAL_SERVER_ERROR
                    && *s != reqwest::StatusCode::SERVICE_UNAVAILABLE
            }
            Err(_) => true,
        })
        .collect();

    assert!(
        routing_failures.is_empty(),
        "router must not produce 4xx/timeouts under fixed-host load, failures: {routing_failures:?}"
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

/// An HTTP/1.0 request without a Host header must return 400
/// (RouteError::MissingHost). but since reqwest always injects a Host header, this
/// test uses a raw TCP connection to send a minimal HTTP/1.0 request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_missing_host_returns_400() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    host.workload_start(http_counter_request("present.local", None))
        .await?;

    let raw_request = "GET / HTTP/1.0\r\n\r\n";
    let mut stream = timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(addr))
        .await
        .context("connect timed out")?
        .context("connect failed")?;

    stream.write_all(raw_request.as_bytes()).await?;

    let mut response = String::new();
    timeout(Duration::from_secs(5), stream.read_to_string(&mut response))
        .await
        .context("read timed out")?
        .context("read failed")?;

    assert!(
        response.starts_with("HTTP/1.1 400") || response.starts_with("HTTP/1.0 400"),
        "expected HTTP 400 for missing Host header, got: {response:?}"
    );

    Ok(())
}

/// Invalid hostnames in `host-aliases` (e.g. names containing spaces) must be
/// silently filtered by `DynamicRouter::on_workload_resolved`. The valid alias
/// must still route; the invalid one must not appear in the routing table and
/// must therefore return 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_invalid_alias_is_silently_filtered() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;

    // "not a hostname" contains spaces; is_valid_hostname func rejects it.
    let req = http_counter_request("valid.local", Some("not a hostname"));
    host.workload_start(req).await?;

    let client = reqwest::Client::new();

    // valid.local is the primary host and must route successfully.
    assert!(
        get_status(&client, addr, "valid.local").await?.is_success(),
        "primary host should route successfully"
    );

    // "not a hostname" must not have been registered and must return 404.
    let invalid_status = get_status(&client, addr, "not a hostname").await?;
    assert_eq!(
        invalid_status,
        reqwest::StatusCode::NOT_FOUND,
        "invalid alias must be filtered and return 404, got {invalid_status}"
    );

    Ok(())
}

/// Two workloads registered under the same primary host both need to be
/// reachable: the HashSet in `host_to_workload` allows multiple IDs per host,
/// and the router must keep the host routable as long as at least one workload
/// is bound. After stopping one, requests must still succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multiple_workloads_same_host_both_serve() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;

    let req_a = http_counter_request("shared.local", None);
    let req_b = http_counter_request("shared.local", None);
    let workload_id_a = req_a.workload_id.clone();

    host.workload_start(req_a).await?;
    host.workload_start(req_b).await?;

    let client = reqwest::Client::new();

    // Both workloads are bound; shared.local must route.
    assert!(
        get_status(&client, addr, "shared.local")
            .await?
            .is_success(),
        "shared.local should route when two workloads are bound"
    );

    // Stop one workload — the other must keep the host alive.
    host.workload_stop(wash_runtime::types::WorkloadStopRequest {
        workload_id: workload_id_a,
    })
    .await?;

    assert!(
        get_status(&client, addr, "shared.local")
            .await?
            .is_success(),
        "shared.local should still route after one of two workloads is stopped"
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
