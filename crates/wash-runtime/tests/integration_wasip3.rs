//! Integration tests for WASIP3 support.
//!
//! Tests that the wasip3 feature flag correctly enables P3 bindings,
//! P2 components still work when P3 is enabled, and the detection
//! logic correctly identifies P3 components.

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
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");

/// Build an engine with wasip3 enabled.
fn engine_with_p3() -> Engine {
    Engine::builder()
        .with_wasip3(true)
        .build()
        .expect("failed to build engine with wasip3")
}

/// Build and start a host with wasip3 enabled and standard plugins.
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

// Engine configuration tests

#[test]
fn test_engine_builder_wasip3_flag() {
    let engine = Engine::builder().with_wasip3(true).build().unwrap();
    assert!(engine.wasip3(), "engine should report wasip3 enabled");

    let engine = Engine::builder().with_wasip3(false).build().unwrap();
    assert!(!engine.wasip3(), "engine should report wasip3 disabled");
}

#[test]
fn test_engine_builder_wasip3_default_off() {
    let engine = Engine::builder().build().unwrap();
    assert!(!engine.wasip3(), "wasip3 should default to disabled");
}

// P3 component detection tests

#[test]
fn test_targets_wasip3_detects_p3_import() {
    // Component that imports a wasi@0.3 interface
    let wat = r#"
        (component
            (import "wasi:cli/environment@0.3.0" (instance))
        )
    "#;
    let wasm = wat::parse_str(wat).expect("failed to parse WAT");

    let engine = engine_with_p3();
    let component =
        wasmtime::component::Component::new(engine.inner(), &wasm).expect("failed to compile");
    assert!(
        wash_runtime::engine::targets_wasip3(&component),
        "should detect @0.3 import"
    );
}

#[test]
fn test_targets_wasip3_detects_p3_handler() {
    // Component that imports the wasi:http/handler@0.3 interface
    let wat = r#"
        (component
            (import "wasi:http/handler@0.3.0" (instance))
        )
    "#;
    let wasm = wat::parse_str(wat).expect("failed to parse WAT");

    let engine = engine_with_p3();
    let component =
        wasmtime::component::Component::new(engine.inner(), &wasm).expect("failed to compile");
    assert!(
        wash_runtime::engine::targets_wasip3(&component),
        "should detect @0.3 import (handler)"
    );
}

#[test]
fn test_targets_wasip3_ignores_p2() {
    // Component that only imports wasi@0.2
    let wat = r#"
        (component
            (import "wasi:http/incoming-handler@0.2.0" (instance))
        )
    "#;
    let wasm = wat::parse_str(wat).expect("failed to parse WAT");

    let engine = engine_with_p3();
    let component =
        wasmtime::component::Component::new(engine.inner(), &wasm).expect("failed to compile");
    assert!(
        !wash_runtime::engine::targets_wasip3(&component),
        "should not detect P2 component as P3"
    );
}

#[test]
fn test_targets_wasip3_ignores_empty() {
    let wat = r#"(component)"#;
    let wasm = wat::parse_str(wat).expect("failed to parse WAT");

    let engine = engine_with_p3();
    let component =
        wasmtime::component::Component::new(engine.inner(), &wasm).expect("failed to compile");
    assert!(
        !wash_runtime::engine::targets_wasip3(&component),
        "empty component should not be detected as P3"
    );
}

#[test]
fn test_targets_wasip3_ignores_non_wasi() {
    // Component with @0.3 in a non-wasi namespace
    let wat = r#"
        (component
            (import "custom:thing/iface@0.3.0" (instance))
        )
    "#;
    let wasm = wat::parse_str(wat).expect("failed to parse WAT");

    let engine = engine_with_p3();
    let component =
        wasmtime::component::Component::new(engine.inner(), &wasm).expect("failed to compile");
    assert!(
        !wash_runtime::engine::targets_wasip3(&component),
        "non-wasi @0.3 should not be detected"
    );
}

// P2 regression: P2 components work with P3 enabled

#[tokio::test]
async fn test_p2_http_component_works_with_p3_enabled() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p2-with-p3-enabled".to_string(),
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
            host_interfaces: http_counter_host_interfaces("p2-test"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("Failed to start P2 workload with P3 enabled")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p2-test")
            .send(),
    )
    .await
    .context("Request timed out")?
    .context("Failed to send request")?;

    assert!(
        response.status().is_success(),
        "P2 component should work with P3 enabled, got status {}",
        response.status()
    );

    let body = response.text().await?;
    assert_eq!(body.trim(), "1", "P2 counter should work normally");

    Ok(())
}

#[tokio::test]
async fn test_p2_concurrent_requests_with_p3_enabled() -> Result<()> {
    let (addr, host) = start_p3_host("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p2-concurrent-p3".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-counter.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_COUNTER_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: http_counter_host_interfaces("concurrent-test"),
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;

    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    for _ in 0..5 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            timeout(
                Duration::from_secs(10),
                client
                    .get(format!("http://{addr}/"))
                    .header("HOST", "concurrent-test")
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
        "at least 3/5 concurrent requests should succeed with P3 enabled, got {successful}"
    );

    Ok(())
}

// Component initialization with P3 linker

#[tokio::test]
async fn test_p3_linker_accepts_p2_component() -> Result<()> {
    // Engine with P3 enabled should still initialize P2 components
    let engine = engine_with_p3();

    let workload = Workload {
        namespace: "test".to_string(),
        name: "p2-on-p3-linker".to_string(),
        annotations: HashMap::new(),
        service: None,
        components: vec![Component {
            name: "http-counter.wasm".to_string(),
            digest: None,
            bytes: bytes::Bytes::from_static(HTTP_COUNTER_WASM),
            local_resources: LocalResources::default(),
            pool_size: 1,
            max_invocations: 100,
        }],
        host_interfaces: http_counter_host_interfaces("linker-test"),
        volumes: vec![],
    };

    let result = engine.initialize_workload("test-id", workload);
    assert!(
        result.is_ok(),
        "P3-enabled engine should accept P2 component: {:?}",
        result.err()
    );

    Ok(())
}

#[tokio::test]
async fn test_p3_disabled_engine_rejects_nothing() -> Result<()> {
    // Engine without P3 should still work fine for P2
    let engine = Engine::builder().with_wasip3(false).build()?;

    let workload = Workload {
        namespace: "test".to_string(),
        name: "p2-no-p3".to_string(),
        annotations: HashMap::new(),
        service: None,
        components: vec![Component {
            name: "http-counter.wasm".to_string(),
            digest: None,
            bytes: bytes::Bytes::from_static(HTTP_COUNTER_WASM),
            local_resources: LocalResources::default(),
            pool_size: 1,
            max_invocations: 100,
        }],
        host_interfaces: http_counter_host_interfaces("no-p3-test"),
        volumes: vec![],
    };

    let result = engine.initialize_workload("test-id", workload);
    assert!(
        result.is_ok(),
        "P2 component on non-P3 engine should work: {:?}",
        result.err()
    );

    Ok(())
}
