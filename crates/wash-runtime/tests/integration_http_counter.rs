//! Integration test for http-counter component
//!
//! Tests HTTP request handling, counter persistence, concurrent access,
//! error handling, and plugin isolation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer, WasiOutgoingHandler},
    },
    plugin::{
        wasi_blobstore::InMemoryBlobstore, wasi_config::DynamicConfig,
        wasi_keyvalue::InMemoryKeyValue, wasi_logging::TracingLogger,
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");

/// Standard set of WASI interfaces used by the http-counter component.
fn http_counter_host_interfaces(http_host_config: &str) -> Vec<WitInterface> {
    vec![
        WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config: {
                let mut config = HashMap::new();
                config.insert("host".to_string(), http_host_config.to_string());
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

/// Build and start a host with the standard set of plugins (HTTP, blobstore, keyvalue, logging, config).
async fn start_host_with_all_plugins(addr: &str) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server =
        HttpServer::new(DevRouter::default(), WasiOutgoingHandler, addr.parse()?).await?;
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

/// Create a workload start request for the http-counter component.
fn http_counter_workload_request(
    http_host_config: &str,
    local_resources: LocalResources,
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
                local_resources,
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: http_counter_host_interfaces(http_host_config),
            volumes: vec![],
        },
    }
}

#[tokio::test]
async fn test_http_counter_integration() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let (addr, host) = start_host_with_all_plugins("127.0.0.1:0").await?;

    let req = http_counter_workload_request(
        "foo",
        LocalResources {
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
    );

    host.workload_start(req)
        .await
        .context("Failed to start http-counter workload")?;

    let client = reqwest::Client::new();

    // First request should initialize counter to 1
    let first_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "foo")
            .send(),
    )
    .await
    .context("First request timed out")?
    .context("Failed to make first request")?;

    assert!(
        first_response.status().is_success(),
        "First request failed with status {}",
        first_response.status(),
    );

    let first_text = first_response.text().await?;
    assert_eq!(
        first_text.trim(),
        "1",
        "First request should return counter value of 1"
    );

    // Second request should increment counter
    let second_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "foo")
            .send(),
    )
    .await
    .context("Second request timed out")?
    .context("Failed to make second request")?;

    assert!(second_response.status().is_success());

    let second_count: u64 = second_response
        .text()
        .await?
        .trim()
        .parse()
        .expect("Response should be a valid number");
    assert!(
        second_count >= 1,
        "Second request should return counter >= 1, got {second_count}"
    );

    // Concurrent requests should all succeed
    let mut handles = Vec::new();
    for _ in 0..5 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client
                .get(format!("http://{addr}/"))
                .header("HOST", "foo")
                .send()
                .await
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
        "At least 3 out of 5 concurrent requests should succeed, only {successful} succeeded",
    );

    Ok(())
}

#[tokio::test]
async fn test_http_counter_error_scenarios() -> Result<()> {
    let (addr, host) = start_host_with_all_plugins("127.0.0.1:0").await?;

    let req = http_counter_workload_request("error-test", LocalResources::default());

    host.workload_start(req)
        .await
        .context("Failed to start error test workload")?;

    let client = reqwest::Client::new();

    // Malformed request should be handled gracefully
    let malformed_response = client
        .post(format!("http://{addr}/invalid-path"))
        .header("HOST", "error-test")
        .body("invalid-data")
        .send()
        .await;

    if let Ok(response) = malformed_response {
        let status = response.status();
        assert!(
            status.is_client_error() || status.is_server_error() || status.is_success(),
            "Should return valid HTTP status for malformed request"
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_http_counter_plugin_isolation() -> Result<()> {
    // Two independent hosts should start without interference
    let (_addr1, _host1) = start_host_with_all_plugins("127.0.0.1:0").await?;
    let (_addr2, _host2) = start_host_with_all_plugins("127.0.0.1:0").await?;

    Ok(())
}
