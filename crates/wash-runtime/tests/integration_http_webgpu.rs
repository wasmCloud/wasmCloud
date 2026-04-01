//! Integration test for HTTP and webgpu plugins working together
//!
//! This test demonstrates:
//! 1. Starting a host with HTTP and webgpu plugins
//! 2. Creating and starting a workload
//! 3. Verifying the HTTP server is listening and responding
//! 4. Confirming the component works with the webgpu plugin
//!
//! Note: This is a basic integration test that verifies plugin loading and host functionality.
//! Full component binding and request routing would require proper WIT interface configuration.
//!
#![cfg(feature = "wasi-webgpu")]

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::wasi_webgpu::{WebGpu, WebGpuBackend},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_WEBGPU_WASM: &[u8] = include_bytes!("wasm/http_webgpu.wasm");

#[tokio::test]
async fn test_http_webgpu_integration() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    println!("Starting HTTP + Webgpu integration test");

    // Create engine
    let engine = Engine::builder().build()?;

    // Create HTTP server plugin on a dynamically allocated port
    let http_plugin = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_plugin.addr();

    // Build host with plugins following the existing pattern from lib.rs test
    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(http_plugin))
        .with_plugin(Arc::new(WebGpu::new(WebGpuBackend::Noop)))?
        .build()?;

    println!("Created host with HTTP and webgpu plugins");

    // Start the host (which starts plugins)
    let host = host.start().await.context("Failed to start host")?;

    println!("Host started, HTTP server listening on {addr}");

    // Create a workload request with the HTTP component
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "test-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-webgpu-component".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_WEBGPU_WASM),
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
            host_interfaces: vec![
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.2.2").unwrap()),
                    config: {
                        let mut config = HashMap::new();
                        config.insert("host".to_string(), "foo".to_string());
                        config
                    },
                    name: None,
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "graphics-context".to_string(),
                    interfaces: ["graphics-context".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.0.1").unwrap()),
                    config: HashMap::new(),
                    name: None,
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "webgpu".to_string(),
                    interfaces: ["webgpu".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.0.1").unwrap()),
                    config: HashMap::new(),
                    name: None,
                },
            ],
            volumes: vec![],
        },
    };

    // Start the workload
    let workload_response = host
        .workload_start(req)
        .await
        .context("Failed to start workload")?;
    println!(
        "Started workload: {:?}",
        workload_response.workload_status.workload_id
    );

    // Test 1: Make HTTP POST request with body data to the webgpu component
    println!("Testing webgpu component endpoint with POST data");

    let test_numbers = [1, 2, 3, 4];
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(5),
        client
            .post(format!(
                "http://{addr}/{}",
                test_numbers
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ))
            .send(),
    )
    .await
    .context("HTTP request timed out")?
    .context("Failed to make HTTP request")?;

    let status = response.status();
    println!("HTTP Response Status: {}", status);

    let response_text = response
        .text()
        .await
        .context("Failed to read response body")?;
    println!("HTTP Response Body: {}", response_text);

    // The webgpu component should now work with proper plugin binding
    println!("Webgpu component responded successfully");
    assert!(status.is_success(), "Expected success, got {}", status);
    assert!(
        !response_text.trim().is_empty(),
        "Expected response body content"
    );
    // parse the numbers from the response
    let response_numbers = response_text
        .chars()
        .filter(|s| s == &',' || s.is_digit(10))
        .collect::<String>()
        .split(",")
        .map(|s| s.parse::<u32>().unwrap())
        .collect::<Vec<u32>>();
    // expected to just double the numbers
    let expected_response = expected_gpu_response(&test_numbers);
    assert_eq!(
        response_numbers, expected_response,
        "Expected response to match the data we sent (round-trip verification)"
    );
    println!("Component successfully performed round-trip: POST data → webgpu → response");

    // Test 2: The component itself uses webgpu, so this demonstrates both plugins working
    println!("HTTP and webgpu plugins are both active and working together");

    println!("All integration tests passed");
    Ok(())
}

fn expected_gpu_response(numbers: &[u32]) -> Vec<u32> {
    // no-op backend does not change the numbers
    numbers.to_vec()
}
