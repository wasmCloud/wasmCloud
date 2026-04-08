//! Integration tests for P2/P3 component matrix.
//!
//! Tests all combinations of WASIP2 and WASIP3 components and services:
//! - P3 HTTP handler (minimal and with plugins)
//! - P3 CLI service
//! - P3 service + P2 component
//! - P2 service + P3 component
//! - Mixed P2/P3 components in same workload
//! - P2 regression with P3 engine enabled

#![cfg(feature = "wasip3")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
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
    types::{Component, LocalResources, Service, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

// P3 fixtures
const HTTP_HANDLER_P3_WASM: &[u8] = include_bytes!("wasm/http_handler_p3.wasm");
const HTTP_BLOBSTORE_P3_WASM: &[u8] = include_bytes!("wasm/http_blobstore_p3.wasm");
const CLI_SERVICE_P3_WASM: &[u8] = include_bytes!("wasm/cli_service_p3.wasm");

// P3 inter-component fixtures
const P3_CALLER_WASM: &[u8] = include_bytes!("wasm/inter_component_call_p3_caller.wasm");
const P3_CALLEE_WASM: &[u8] = include_bytes!("wasm/inter_component_call_p3_callee.wasm");

// P2 fixtures
const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");
// P2 inter-component fixtures
const P2_CALLER_WASM: &[u8] = include_bytes!("wasm/inter_component_call_caller.wasm");
const P2_MIDDLEWARE_WASM: &[u8] = include_bytes!("wasm/inter_component_call_middleware.wasm");
const P2_CALLEE_WASM: &[u8] = include_bytes!("wasm/inter_component_call_callee.wasm");

fn engine_with_p3() -> Engine {
    Engine::builder()
        .with_wasip3(true)
        .build()
        .expect("failed to build engine with wasip3")
}

async fn start_p3_host(addr: &str) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = engine_with_p3();
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

fn p3_http_host_interfaces(host_header: &str) -> Vec<WitInterface> {
    vec![WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: ["incoming-handler".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.2.2").unwrap()),
        config: {
            let mut config = HashMap::new();
            config.insert("host".to_string(), host_header.to_string());
            config
        },
        name: None,
    }]
}

fn http_counter_host_interfaces(host_header: &str) -> Vec<WitInterface> {
    vec![
        WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config: {
                let mut config = HashMap::new();
                config.insert("host".to_string(), host_header.to_string());
                config
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

fn p3_http_blobstore_host_interfaces(host_header: &str) -> Vec<WitInterface> {
    vec![
        WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config: {
                let mut config = HashMap::new();
                config.insert("host".to_string(), host_header.to_string());
                config
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
    ]
}

// =============================================================================
// Test 1: P3 HTTP handler serves a request
// =============================================================================

#[tokio::test]
async fn test_p3_http_handler_serves_request() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-http-handler".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-handler-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_HANDLER_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: p3_http_host_interfaces("p3-handler"),
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-handler")
            .send(),
    )
    .await??;

    assert!(
        response.status().is_success(),
        "P3 HTTP handler should return 200, got {}",
        response.status()
    );

    let body = response.text().await?;
    assert_eq!(body, "hello from p3", "P3 handler body mismatch");

    Ok(())
}

// =============================================================================
// Test 2: P3 HTTP handler with blobstore plugin
// =============================================================================

#[tokio::test]
async fn test_p3_http_blobstore() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-http-blobstore".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-blobstore-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_BLOBSTORE_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: p3_http_blobstore_host_interfaces("p3-blobstore"),
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-blobstore")
            .send(),
    )
    .await??;

    assert!(
        response.status().is_success(),
        "P3 HTTP blobstore handler should return 200, got {}",
        response.status()
    );

    let body = response.text().await?;
    assert_eq!(
        body, "hello from p3 blobstore",
        "P3 blobstore body should echo stored data"
    );

    Ok(())
}

// =============================================================================
// Test 3: P3 HTTP handler handles concurrent requests
// =============================================================================

#[tokio::test]
async fn test_p3_http_concurrent_requests() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-http-concurrent".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-handler-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_HANDLER_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: p3_http_host_interfaces("p3-concurrent"),
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    for _ in 0..5 {
        let client = client.clone();
        let addr = addr;
        handles.push(tokio::spawn(async move {
            timeout(
                Duration::from_secs(10),
                client
                    .get(format!("http://{addr}/"))
                    .header("HOST", "p3-concurrent")
                    .send(),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        }));
    }

    let mut successful = 0;
    for handle in handles {
        if handle.await.unwrap_or(false) {
            successful += 1;
        }
    }
    assert!(
        successful >= 3,
        "at least 3/5 concurrent P3 requests should succeed, got {successful}"
    );

    Ok(())
}

// =============================================================================
// Test 4: P3 CLI service runs
// =============================================================================

#[tokio::test]
async fn test_p3_service_runs() -> Result<()> {
    let engine = engine_with_p3();
    let host = HostBuilder::new().with_engine(engine).build()?;
    let host = host.start().await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-service".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(CLI_SERVICE_P3_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("Failed to start P3 service workload")?;

    // Give the service a moment to run
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok(())
}

// =============================================================================
// Test 5: P3 caller → P2 middleware → P2 callee (inter-component linking)
// P3 HTTP handler calls into P2 component chain via shared WIT interface
// =============================================================================

fn inter_component_host_interfaces(host_header: &str) -> Vec<WitInterface> {
    vec![
        WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: None,
            config: {
                let mut config = HashMap::new();
                config.insert("host".to_string(), host_header.to_string());
                config
            },
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
    ]
}

#[tokio::test]
async fn test_p3_caller_p2_middleware_p2_callee() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    // P3 caller imports middleware, P2 middleware imports receiver, P2 callee exports receiver
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-p2-p2-link".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "p3-caller".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(P3_CALLER_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: "p2-middleware".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(P2_MIDDLEWARE_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: "p2-callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(P2_CALLEE_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
            ],
            host_interfaces: inter_component_host_interfaces("p3-p2-p2"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("P3 caller → P2 middleware → P2 callee workload should start")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-p2-p2")
            .send(),
    )
    .await??;

    assert!(
        response.status().is_success(),
        "P3→P2→P2 inter-component call should succeed, got {}",
        response.status()
    );

    let body = response.text().await?;
    assert_eq!(body, "p3-caller-ok");

    Ok(())
}

// =============================================================================
// Test 6: P2 caller → P2 middleware → P3 callee (inter-component linking)
// P2 HTTP handler, P3 component at the end of the chain
// =============================================================================

#[tokio::test]
async fn test_p2_caller_p2_middleware_p3_callee() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    // P2 caller imports middleware, P2 middleware imports receiver, P3 callee exports receiver
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p2-p2-p3-link".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "p2-caller".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(P2_CALLER_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: "p2-middleware".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(P2_MIDDLEWARE_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: "p3-callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(P3_CALLEE_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
            ],
            host_interfaces: inter_component_host_interfaces("p2-p2-p3"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("P2 caller → P2 middleware → P3 callee workload should start")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p2-p2-p3")
            .send(),
    )
    .await??;

    assert!(
        response.status().is_success(),
        "P2→P2→P3 inter-component call should succeed, got {}",
        response.status()
    );

    Ok(())
}

// =============================================================================
// Test 7: P3 caller → P2 middleware → P3 callee (inter-component linking)
// P3 components on both ends, P2 middleware in the middle
// =============================================================================

#[tokio::test]
async fn test_p3_caller_p2_middleware_p3_callee() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-p2-p3-link".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "p3-caller".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(P3_CALLER_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: "p2-middleware".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(P2_MIDDLEWARE_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: "p3-callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(P3_CALLEE_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
            ],
            host_interfaces: inter_component_host_interfaces("p3-p2-p3"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("P3 caller → P2 middleware → P3 callee workload should start")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-p2-p3")
            .send(),
    )
    .await??;

    assert!(
        response.status().is_success(),
        "P3→P2→P3 inter-component call should succeed, got {}",
        response.status()
    );

    let body = response.text().await?;
    assert_eq!(body, "p3-caller-ok");

    Ok(())
}

// =============================================================================
// Test 8: Full P3 workload (P3 service + P3 HTTP component)
// =============================================================================

#[tokio::test]
async fn test_all_p3_workload() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "all-p3".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(CLI_SERVICE_P3_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            components: vec![Component {
                name: "http-blobstore-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_BLOBSTORE_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: p3_http_blobstore_host_interfaces("all-p3"),
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "all-p3")
            .send(),
    )
    .await??;

    assert!(
        response.status().is_success(),
        "All-P3 workload should serve requests, got {}",
        response.status()
    );

    let body = response.text().await?;
    assert_eq!(body, "hello from p3 blobstore");

    Ok(())
}

// =============================================================================
// Test 9: P2 regression - P2 component works correctly with P3 engine enabled
// =============================================================================

#[tokio::test]
async fn test_p2_regression_with_p3_enabled() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p2-regression".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-counter.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_COUNTER_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 256,
                    cpu_limit: 1,
                    config: HashMap::from([
                        ("test_key".to_string(), "test_value".to_string()),
                        ("counter_enabled".to_string(), "true".to_string()),
                    ]),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: Default::default(),
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: http_counter_host_interfaces("p2-regression"),
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p2-regression")
            .send(),
    )
    .await??;

    assert!(
        response.status().is_success(),
        "P2 component should work with P3 engine, got {}",
        response.status()
    );

    let body = response.text().await?;
    assert_eq!(body.trim(), "1", "P2 counter should work normally");

    Ok(())
}
