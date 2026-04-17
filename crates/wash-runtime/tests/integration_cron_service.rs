//! Integration test for cron-service component
//!
//! This test demonstrates:
//! 1. Starting a host with no plugins
//! 2. Creating and starting a workload with a service that runs periodically
//! 3. Capturing and verifying stderr output from the service and component

use anyhow::{Context, Result};
use gag::BufferRedirect;
use std::collections::HashMap;
use std::io::Read;

use wash_runtime::{
    engine::Engine,
    host::{HostApi, HostBuilder},
    types::{Component, Service, Workload, WorkloadStartRequest},
};

const CRON_SERVICE_WASM: &[u8] = include_bytes!("wasm/cron_service.wasm");

const CRON_COMPONENT_WASM: &[u8] = include_bytes!("wasm/cron_component.wasm");

#[tokio::test]
async fn test_cron_service_integration() -> Result<()> {
    // Capture stderr to verify WASI component output
    let mut stderr_capture = BufferRedirect::stderr().expect("failed to redirect stderr");

    println!("Starting cron-service integration test");

    // Create engine
    let engine = Engine::builder().build()?;

    // Build host with no plugins
    let host = HostBuilder::new().with_engine(engine.clone()).build()?;

    println!("Created host with no plugins");

    // Start the host
    let host = host.start().await.context("Failed to start host")?;
    println!("Host started");

    // Create a workload request with a service and component
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "cron-service-workload".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(CRON_SERVICE_WASM),
                local_resources: Default::default(),
                max_restarts: 0,
            }),
            components: vec![Component {
                name: "cron-component".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(CRON_COMPONENT_WASM),
                local_resources: Default::default(),
                max_invocations: 1,
                pool_size: 0,
            }],
            host_interfaces: vec![],
            volumes: vec![],
        },
    };

    // Start the workload
    let _workload_response = host
        .workload_start(req)
        .await
        .context("Failed to start cron-service workload")?;

    println!("Workload started successfully");
    println!("Waiting for service to execute (5 seconds)...");

    // Wait for service to execute multiple times (at least 3 times with 1 second intervals)
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Get captured stderr
    let mut output = String::new();
    stderr_capture
        .read_to_string(&mut output)
        .expect("failed to read captured stderr");

    println!("\n=== Captured stderr ===");
    println!("{}", output);
    println!("=====================\n");

    // Verify expected messages
    assert!(
        output.contains("Starting cron-service with 1 second intervals..."),
        "Expected to find 'Starting cron-service with 1 second intervals...' in stderr.\nCaptured stderr:\n{}",
        output
    );

    // Check that "Hello from the cron-component!" appears at least 3 times
    let hello_count = output.matches("Hello from the cron-component!").count();
    assert!(
        hello_count >= 3,
        "Expected at least 3 'Hello from the cron-component!' messages, but found {}.\nCaptured stderr:\n{}",
        hello_count,
        output
    );

    println!("✓ Found cron service start message");
    println!(
        "✓ Found {} cron-component messages (expected at least 3)",
        hello_count
    );

    Ok(())
}
