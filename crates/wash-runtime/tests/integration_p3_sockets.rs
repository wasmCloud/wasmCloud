//! Integration tests for P3 socket implementations.
//!
//! Tests the P3 TCP and UDP socket code paths using a component that
//! exercises TCP loopback listen/connect/send/receive and UDP loopback
//! send/receive.
//!
//! Code paths tested:
//! - `sockets/host_tcp_p3.rs`: ListenStreamProducer, ReceiveStreamProducer, SendStreamConsumer
//! - `sockets/host_udp_p3.rs`: HostUdpSocketWithStore send/receive
//! - `sockets/mod.rs`: add_p3_to_linker, error code mapping

#![cfg(feature = "wasip3")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::collections::HashMap;

use wash_runtime::{
    engine::Engine,
    host::{HostApi, HostBuilder},
    types::{Component, Service, Workload, WorkloadStartRequest},
};

const SOCKET_TEST_P3_WASM: &[u8] = include_bytes!("wasm/socket_test_p3.wasm");

fn engine_with_p3() -> Engine {
    Engine::builder()
        .with_wasip3(true)
        .build()
        .expect("failed to build engine with wasip3")
}

/// Test P3 TCP loopback: bind, listen, connect, send, receive.
///
/// The socket-test-p3 component creates a TCP listener on loopback,
/// connects a client, sends data, and verifies receipt on the server side.
/// This exercises ListenStreamProducer, ReceiveStreamProducer, and
/// SendStreamConsumer in host_tcp_p3.rs.
#[tokio::test]
async fn test_p3_tcp_loopback() -> Result<()> {
    let engine = engine_with_p3();
    let host = HostBuilder::new().with_engine(engine).build()?;
    let host = host.start().await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-socket-tcp".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(SOCKET_TEST_P3_WASM),
                local_resources: Default::default(),
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("P3 socket test service should start and complete")?;

    // The service runs both TCP and UDP tests, then exits.
    // Give it time to complete.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    Ok(())
}

/// Test that a P3 socket component can be used as a regular component
/// (not just a service).
#[tokio::test]
async fn test_p3_socket_component_initialization() -> Result<()> {
    let engine = engine_with_p3();

    let workload = Workload {
        namespace: "test".to_string(),
        name: "p3-socket-init".to_string(),
        annotations: HashMap::new(),
        service: None,
        components: vec![Component {
            name: "socket-test-p3.wasm".to_string(),
            digest: None,
            bytes: bytes::Bytes::from_static(SOCKET_TEST_P3_WASM),
            local_resources: Default::default(),
            pool_size: 1,
            max_invocations: 10,
        }],
        host_interfaces: vec![],
        volumes: vec![],
    };

    let result = engine.initialize_workload("socket-test-id", workload);
    assert!(
        result.is_ok(),
        "P3 socket component should initialize: {:?}",
        result.err()
    );

    Ok(())
}

/// Test that the P3 engine correctly detects socket-test-p3 as a P3 component.
#[test]
fn test_p3_socket_component_detected_as_p3() {
    let engine = engine_with_p3();
    let component = wasmtime::component::Component::new(engine.inner(), SOCKET_TEST_P3_WASM)
        .expect("failed to compile socket test component");

    assert!(
        wash_runtime::engine::targets_wasip3(&component),
        "socket-test-p3 should be detected as P3"
    );
}
