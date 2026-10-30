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

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::oneshot, time::timeout};

use wash_runtime::{
    engine::{Engine, workload::WorkloadItem},
    host::{HostApi, HostBuilder},
    plugin::{HostPlugin, WitInterfaces},
    types::{Component, Service, Workload, WorkloadStartRequest, WorkloadStopRequest},
    wit::WitWorld,
};

const SOCKET_TEST_P3_WASM: &[u8] = include_bytes!("wasm/socket_test_p3.wasm");

fn engine_with_p3() -> Engine {
    Engine::builder()
        .build()
        .expect("failed to build engine with wasip3")
}

struct Completion(Arc<Mutex<Option<oneshot::Sender<()>>>>);

#[async_trait::async_trait]
impl HostPlugin for Completion {
    fn id(&self) -> &'static str {
        "socket-completion"
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: ["wasmcloud:test-p3/completion@0.1.0".into()].into(),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        _: WitInterfaces<'_>,
    ) -> Result<()> {
        let done = self.0.clone();
        item.linker()?
            .instance("wasmcloud:test-p3/completion@0.1.0")?
            .func_wrap("done", move |_, (): ()| {
                if let Some(tx) = done.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                Ok(())
            })?;
        Ok(())
    }
}

/// The guest reports completion only after its TCP and UDP assertions pass.
#[tokio::test]
async fn test_p3_tcp_and_udp_loopback() -> Result<()> {
    let engine = engine_with_p3();
    let (done_tx, done_rx) = oneshot::channel();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_plugin(Arc::new(Completion(Arc::new(Mutex::new(Some(done_tx))))))?
        .build()?;
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
            host_interfaces: vec!["wasmcloud:test-p3/completion@0.1.0".into()],
            volumes: vec![],
        },
    };

    let workload_id = req.workload_id.clone();
    host.workload_start(req)
        .await
        .context("P3 socket test service should start")?;

    // The plugin holds the sender for the host's lifetime, so a guest that traps
    // before `done` surfaces as this timeout; the trap itself is in the log.
    timeout(Duration::from_secs(30), done_rx)
        .await
        .context("P3 socket guest did not report passing TCP/UDP assertions")?
        .expect("the completion plugin outlives the guest");
    host.workload_stop(WorkloadStopRequest { workload_id })
        .await?;

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
            max_concurrency: 1,
            ..Default::default()
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
