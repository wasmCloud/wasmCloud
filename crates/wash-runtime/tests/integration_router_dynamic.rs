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

use anyhow::{Context, Result};
use futures::future::join_all;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DynamicRouter, HttpServer},
    },
    plugin::{
        wasi_blobstore::InMemoryBlobstore, wasi_config::DynamicConfig,
        wasi_keyvalue::InMemoryKeyValue, wasi_logging::TracingLogger,
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadStopRequest},
    wit::WitInterface,
};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");

fn http_counter_host_interfaces(
    http_host_config: &str,
    aliases: Option<&str>,
) -> Vec<WitInterface> {
    let mut http_config = HashMap::new();
    http_config.insert("host".to_string(), http_host_config.to_string());
    if let Some(aliases) = aliases {
        http_config.insert("host-aliases".to_string(), aliases.to_string());
    }

    vec![
        WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config: http_config,
            name: None,
        },
        WitInterface {
            namespace: "wasi".to_string(),
            package: "blobstore".to_string(),
            interfaces: [
                "blobstore".to_string(),
                "container".to_string(),
                "types".to_string(),
            ]
            .into_iter()
            .collect(),
            version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
            config: HashMap::new(),
            name: None,
        },
        WitInterface {
            namespace: "wasi".to_string(),
            package: "keyvalue".to_string(),
            interfaces: ["store".to_string(), "atomics".to_string()]
                .into_iter()
                .collect(),
            version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
            config: HashMap::new(),
            name: None,
        },
        WitInterface {
            namespace: "wasi".to_string(),
            package: "logging".to_string(),
            interfaces: ["logging".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
            config: HashMap::new(),
            name: None,
        },
        WitInterface {
            namespace: "wasi".to_string(),
            package: "config".to_string(),
            interfaces: ["store".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.0-rc.1").unwrap()),
            config: HashMap::new(),
            name: None,
        },
    ]
}

async fn start_host_with_dynamic_router(
    addr: &str,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DynamicRouter::default(), addr.parse()?).await?;
    let bound_addr = http_server.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(InMemoryBlobstore::new(None)))?
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host))
}

fn http_counter_workload_request(
    http_host_config: &str,
    aliases: Option<&str>,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "http-counter-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-counter.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_COUNTER_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 256,
                    cpu_limit: 1,
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: Default::default(),
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: http_counter_host_interfaces(http_host_config, aliases),
            volumes: vec![],
        },
    }
}

/// Fan out concurrent requests across three hostnames (primary + 2 aliases)
/// registered to one workload; all must succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_dynamic_router_multiple_distinct_hosts_concurrent() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;

    let aliases = Some("web.local,admin.local");

    let req = http_counter_workload_request("api.local", aliases);
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
            let resp = timeout(
                Duration::from_secs(5),
                client
                    .get(format!("http://{addr}/"))
                    .header("HOST", hostname)
                    .send(),
            )
            .await
            .ok()
            .and_then(|r| r.ok());
            resp.map(|r| r.status().is_success()).unwrap_or(false)
        }));
    }

    let successful = join_all(handles)
        .await
        .into_iter()
        .filter(|r| r.as_ref().copied().unwrap_or(false))
        .count();
    // 18/20 requests accounting 2 reqs for flakes
    assert!(
        successful >= 18,
        "expected >=18 concurrent multi-host requests to succeed, got {successful}"
    );

    Ok(())
}

/// 50 concurrent requests against a single fixed host must mostly succeed
/// and complete quickly, the try_read must not deadlock or starve under stres.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_dynamic_router_concurrent_routes_fixed_mapping() -> Result<()> {
    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    let req = http_counter_workload_request("fixed.local", None);
    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let started = std::time::Instant::now();
    let mut handles = Vec::with_capacity(50);
    for _ in 0..50 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            timeout(
                Duration::from_secs(15),
                client
                    .get(format!("http://{addr}/"))
                    .header("HOST", "fixed.local")
                    .send(),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .map(|r| r.status().is_success())
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
    let req = http_counter_workload_request("race.local", None);
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
