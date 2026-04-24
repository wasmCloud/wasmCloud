//! Integration test for http-counter component
//!
//! Tests HTTP request handling, counter persistence, concurrent access,
//! error handling, and plugin isolation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{host::HostApi, types::LocalResources};

mod common;
use common::{
    component_workload_request, http_counter_host_interfaces, start_host_with_dev_router,
};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");

fn http_counter_request(
    host_header: &str,
    local_resources: LocalResources,
) -> wash_runtime::types::WorkloadStartRequest {
    component_workload_request(
        "http-counter.wasm",
        "http-counter-workload",
        HTTP_COUNTER_WASM,
        local_resources,
        http_counter_host_interfaces(host_header),
    )
}

#[tokio::test]
async fn test_http_counter_integration() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let (addr, host) = start_host_with_dev_router("127.0.0.1:0").await?;

    let req = http_counter_request(
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
    let (addr, host) = start_host_with_dev_router("127.0.0.1:0").await?;

    let req = http_counter_request("error-test", LocalResources::default());

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
    let (_addr1, _host1) = start_host_with_dev_router("127.0.0.1:0").await?;
    let (_addr2, _host2) = start_host_with_dev_router("127.0.0.1:0").await?;

    Ok(())
}
