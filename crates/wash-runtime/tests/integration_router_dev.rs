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

use anyhow::{Context, Result};
use futures::future::join_all;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::{
        wasi_blobstore::InMemoryBlobstore, wasi_config::DynamicConfig,
        wasi_keyvalue::InMemoryKeyValue, wasi_logging::TracingLogger,
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadStopRequest},
    wit::WitInterface,
};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");
const HTTP_HANDLER_P2_WASM: &[u8] = include_bytes!("wasm/http_handler_p2.wasm");

fn http_counter_host_interfaces(http_host_config: &str) -> Vec<WitInterface> {
    vec![
        WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config: {
                let mut c = HashMap::new();
                c.insert("host".to_string(), http_host_config.to_string());
                c
            },
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

async fn start_host_with_dev_router(addr: &str) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), addr.parse()?).await?;
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

fn http_counter_workload_request(http_host_config: &str) -> WorkloadStartRequest {
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
            host_interfaces: http_counter_host_interfaces(http_host_config),
            volumes: vec![],
        },
    }
}

fn http_handler_p2_host_interfaces(http_host_config: &str) -> Vec<WitInterface> {
    vec![WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: ["incoming-handler".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.2.2").unwrap()),
        config: {
            let mut c = HashMap::new();
            c.insert("host".to_string(), http_host_config.to_string());
            c
        },
        name: None,
    }]
}

fn http_handler_p2_workload_request(http_host_config: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "http-handler-p2-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-handler-p2.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_HANDLER_P2_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 128,
                    cpu_limit: 1,
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: Default::default(),
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: http_handler_p2_host_interfaces(http_host_config),
            volumes: vec![],
        },
    }
}

async fn get_status(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
) -> Result<reqwest::StatusCode> {
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", host_header)
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    Ok(response.status())
}

async fn get_status_and_body(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", host_header)
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

fn is_unrouted(status: reqwest::StatusCode) -> bool {
    !status.is_success()
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
    let req_a = http_counter_workload_request("a");
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
    let req_b = http_handler_p2_workload_request("b");
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

    let req_a = http_counter_workload_request("a");
    let id_a = req_a.workload_id.clone();
    host.workload_start(req_a).await?;

    // Starting B makes B the "last" workload. A is still registered but
    // is no longer the routing target.
    let req_b = http_counter_workload_request("b");
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

    // Stopping B clears the binding so subsequent requests return 5xx.
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

    let req_a = http_counter_workload_request("a");
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
    let req_b = http_counter_workload_request("b");
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
