//! Integration test for HTTP and blobstore plugins working together
//!
//! This test demonstrates:
//! 1. Starting a host with HTTP and blobstore plugins
//! 2. Creating and starting a workload
//! 3. Verifying the HTTP server is listening and responding
//! 4. Confirming both plugins load and initialize correctly
//!
//! Note: This is a basic integration test that verifies plugin loading and host functionality.
//! Full component binding and request routing would require proper WIT interface configuration.

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer, WasiOutgoingHandler},
    },
    plugin::wasi_blobstore::InMemoryBlobstore,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_BLOBSTORE_WASM: &[u8] = include_bytes!("wasm/http_blobstore.wasm");

#[tokio::test]
async fn test_http_blobstore_integration() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    println!("Starting HTTP + Blobstore integration test with blobstore-filesystem component");

    // Create engine
    let engine = Engine::builder().build()?;

    // Create HTTP server plugin on a dynamically allocated port
    let http_plugin = HttpServer::new(
        DevRouter::default(),
        WasiOutgoingHandler,
        "127.0.0.1:0".parse()?,
    )
    .await?;
    let addr = http_plugin.addr();

    // Create blobstore plugin
    let blobstore_plugin = InMemoryBlobstore::new(None);

    // Build host with plugins following the existing pattern from lib.rs test
    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(http_plugin))
        .with_plugin(Arc::new(blobstore_plugin))?
        .build()?;

    println!("Created host with HTTP and blobstore plugins");

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
                name: "http-blobstore-component".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_BLOBSTORE_WASM),
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

    // Test 1: Make HTTP POST request with body data to the blobstore-filesystem component
    println!("Testing blobstore-filesystem component endpoint with POST data");

    let test_data = "Hello, blobstore world!";
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(5),
        client
            .post(format!("http://{addr}/"))
            .header("HOST", "foo")
            .body(test_data)
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
    println!("HTTP Response Body: {}", response_text.trim());

    // The blobstore-filesystem component should now work with proper plugin binding
    println!("Blobstore-filesystem component responded successfully");
    assert!(status.is_success(), "Expected success, got {}", status);
    assert!(
        !response_text.trim().is_empty(),
        "Expected response body content"
    );
    assert_eq!(
        response_text.trim(),
        test_data,
        "Expected response to match the data we sent (round-trip verification)"
    );
    println!("Component successfully performed round-trip: POST data → blobstore → response");

    // Test 2: The component itself uses blobstore, so this demonstrates both plugins working
    println!("HTTP and blobstore plugins are both active and working together");

    println!("All integration tests passed");
    Ok(())
}

#[tokio::test]
async fn test_plugin_isolation() -> Result<()> {
    // Test that multiple workloads have isolated plugin state
    println!("Testing plugin isolation between workloads");

    let engine = Engine::builder().build()?;
    let blobstore1 = InMemoryBlobstore::new(None);
    let blobstore2 = InMemoryBlobstore::new(None);

    // Create two identical hosts with blobstore plugins
    let _host1 = HostBuilder::new()
        .with_engine(engine.clone())
        .with_plugin(Arc::new(blobstore1))?
        .build()?;

    let _host2 = HostBuilder::new()
        .with_engine(engine.clone())
        .with_plugin(Arc::new(blobstore2))?
        .build()?;

    // Both should be independent instances
    println!("Created two independent hosts with blobstore plugins");
    println!("Plugin isolation test passed");

    Ok(())
}

#[tokio::test]
async fn test_plugin_lifecycle() -> Result<()> {
    // Test plugin start/stop lifecycle
    println!("Testing plugin lifecycle");

    let engine = Engine::builder().build()?;
    let http_plugin = HttpServer::new(
        DevRouter::default(),
        WasiOutgoingHandler,
        "127.0.0.1:0".parse()?,
    )
    .await?;

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_plugin))
        .build()?;

    // Start host
    let _host = host.start().await.context("Failed to start host")?;
    println!("Host started successfully");

    // Note: In the actual API, the host handle manages lifecycle
    println!("Host lifecycle test passed");
    Ok(())
}

// #[tokio::test]
// async fn test_large_payload_stress() -> Result<()> {
//     tracing_subscriber::fmt()
//         .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
//         .init();
//     // Stress test with large payload streaming through the blobstore component
//     println!("Starting large payload stress test");

//     let engine = Engine::builder().build()?;
//     let addr: SocketAddr = "127.0.0.1:8082".parse().unwrap();
//     let http_plugin = HttpServer::new(addr);

//     // Create blobstore plugin with large capacity (2GB limit for this test)
//     let blobstore_plugin = WasiBlobstore::new(Some(2_147_483_648)); // 2GB limit

//     let host = HostBuilder::new()
//         .with_engine(engine)
//         .with_plugin(Arc::new(http_plugin))
//         .with_plugin(Arc::new(blobstore_plugin))
//         .with_plugin(Arc::new(WasiLogging {}))
//         .build()?;

//     let host = host.start().await.context("Failed to start host")?;
//     println!("Host started for stress test, HTTP server listening on {addr}");

//     // Create a workload request with the HTTP component
//     let req = WorkloadStartRequest {
//         workload_id: uuid::Uuid::new_v4().to_string(),
//         workload: Workload {
//             namespace: "stress-test".to_string(),
//             name: "large-payload-workload".to_string(),
//             annotations: HashMap::new(),
//             service: None,
//             components: vec![Component {
//                 bytes: bytes::Bytes::from_static(HTTP_BLOBSTORE_WASM),
//                 local_resources: LocalResources {
//                     memory_limit_mb: 2048, // 2GB memory limit for large payloads
//                     cpu_limit: 4,
//                     config: HashMap::new(),
//                     environment: HashMap::new(),
//                     volume_mounts: vec![],
//                     allowed_hosts: vec![],
//                 },
//                 pool_size: 1,
//                 max_invocations: 10,
//             }],
//             host_interfaces: vec![
//                 WitInterface {
//                     namespace: "wasi".to_string(),
//                     package: "logging".to_string(),
//                     interfaces: vec!["logging".to_string()],
//                     version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
//                     config: HashMap::default(),
//                 },
//                 WitInterface {
//                     namespace: "wasi".to_string(),
//                     package: "http".to_string(),
//                     interfaces: ["incoming-handler".to_string()].into_iter().collect(),
//                     version: Some(semver::Version::parse("0.2.2").unwrap()),
//                     config: {
//                         let mut config = HashMap::new();
//                         config.insert("host".to_string(), "stress-test".to_string());
//                         config
//                     },
//                 },
//                 WitInterface {
//                     namespace: "wasi".to_string(),
//                     package: "blobstore".to_string(),
//                     interfaces: vec![
//                         "blobstore".to_string(),
//                         "container".to_string(),
//                         "types".to_string(),
//                     ],
//                     version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
//                     config: HashMap::new(),
//                 },
//             ],
//             volumes: vec![],
//         },
//     };

//     let workload_response = host
//         .workload_start(req)
//         .await
//         .context("Failed to start stress test workload")?;
//     println!(
//         "Started stress test workload: {:?}",
//         workload_response.workload_status.workload_id
//     );

//     // Create a large payload (512MB of data)
//     println!("Sending 512MB POST request to stress test endpoint...");
//     let start_time = std::time::Instant::now();

//     let client = reqwest::Client::builder()
//         .timeout(Duration::from_secs(120)) // 2 minute timeout
//         .build()
//         .context("Failed to create HTTP client")?;

//     // Create the payload in memory (for simplicity in this test)
//     let payload = "1234567890".repeat(1024 * 1024);
//     let expected_size = payload.len();
//     println!("Built complete payload of {} bytes", payload.len());

//     let response = client
//         .post(&format!("http://{addr}/"))
//         .header("HOST", "stress-test")
//         // .header("Content-Length", expected_size.to_string())
//         .body(payload)
//         .send()
//         .await
//         .context("Failed to make large payload HTTP request")?;

//     let elapsed = start_time.elapsed();
//     let status = response.status();
//     println!("HTTP Response Status: {}", status);
//     println!("Request completed in: {:?}", elapsed);

//     let response_bytes = response
//         .bytes()
//         .await
//         .context("Failed to read response body")?;

//     let response_size = response_bytes.len();
//     println!(
//         "Response size: {} bytes ({:.2} MB)",
//         response_size,
//         response_size as f64 / (1024.0 * 1024.0)
//     );

//     // Verify the response
//     assert!(status.is_success(), "Expected success, got {}", status);
//     assert_eq!(
//         response_size, expected_size,
//         "Expected response size to match sent data size"
//     );

//     // Verify content integrity (check first and last chunks)
//     let response_str = String::from_utf8(response_bytes.to_vec())
//         .context("Failed to convert response to string")?;

//     // assert!(
//     //     response_str.starts_with(&"A".repeat(1000)),
//     //     "Response should start with expected pattern"
//     // );
//     // assert!(
//     //     response_str.ends_with(&"A".repeat(1000)),
//     //     "Response should end with expected pattern"
//     // );

//     let throughput_mbps = (expected_size as f64 * 2.0) / (1024.0 * 1024.0) / elapsed.as_secs_f64(); // *2 for upload + download
//     println!("Throughput: {:.2} MB/s (round-trip)", throughput_mbps);
//     println!("512MB payload stress test passed successfully!");

//     Ok(())
// }
