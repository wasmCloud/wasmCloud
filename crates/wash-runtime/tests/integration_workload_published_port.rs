//! A workload's service listens on a real port.
//!
//! This is #5352's headline: the service binds `127.0.0.1:N` inside the
//! workload's virtual loopback exactly as it does today, and the host binds a
//! real port and splices accepted connections into it. The guest code does not
//! change.
//!
//! It reuses the publisher host component plugins proved out first, so what is
//! new here is only the declaration path — `service.ports` through the wire —
//! and that the workload's *shared* virtual network is the splice target.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use wash_runtime::engine::Engine;
use wash_runtime::host::declared_port::{DeclaredPort, Protocol};
use wash_runtime::host::ports::{PortTable, PublishConfig, PublishContext};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::types::{
    LocalResources, Service, Workload, WorkloadStartRequest, WorkloadStopRequest,
};

const SOCKET_ECHO_WASM: &[u8] = include_bytes!("wasm/socket_echo_plugin.wasm");

/// The port the echo fixture binds inside the virtual network. The fixture is
/// shared with the plugin tests; run as a workload service its `cli/run` export
/// is the service's long-running work, which is exactly the shape a service has.
const ECHO_VIRTUAL_PORT: u16 = 50051;

async fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn enabled_config() -> PublishConfig {
    PublishConfig {
        enabled: true,
        readiness_timeout: Duration::from_secs(10),
        ..Default::default()
    }
}

async fn start_host(table: Arc<PortTable>, config: PublishConfig) -> Result<impl HostApi> {
    let engine = Engine::builder().build()?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_publish_context(PublishContext::new(table, config))
        .build()?;
    host.start().await.context("failed to start host")
}

fn service_workload(ports: Vec<DeclaredPort>) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "echo-service".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(SOCKET_ECHO_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
                ports,
            }),
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    }
}

fn published(host_port: u16) -> DeclaredPort {
    DeclaredPort {
        name: "echo".into(),
        port: ECHO_VIRTUAL_PORT,
        protocol: Protocol::Tcp,
        publish: Some(host_port),
        bind: None,
    }
}

/// The headline: an external client reaches a workload's service, and the
/// service's own code binds nothing but `127.0.0.1`.
#[tokio::test(flavor = "multi_thread")]
async fn an_external_client_reaches_a_workload_service() -> Result<()> {
    let host_port = free_port().await?;
    let h = start_host(PortTable::new(), enabled_config()).await?;
    h.workload_start(service_workload(vec![published(host_port)]))
        .await?;

    let real_addr: std::net::SocketAddr = format!("127.0.0.1:{host_port}").parse()?;
    let mut client = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(real_addr))
        .await
        .context("connecting to the published port timed out")??;

    client.write_all(b"workload").await?;
    let mut buf = vec![0u8; b"workload".len()];
    tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut buf))
        .await
        .context("waiting for the service's echo timed out")??;
    assert_eq!(&buf, b"workload");
    Ok(())
}

/// Stopping the workload closes its listeners and frees the port table entry —
/// the exposure is revoked with the workload, not left behind.
#[tokio::test(flavor = "multi_thread")]
async fn stopping_the_workload_revokes_the_exposure() -> Result<()> {
    let host_port = free_port().await?;
    let table = PortTable::new();
    let h = start_host(Arc::clone(&table), enabled_config()).await?;

    let request = service_workload(vec![published(host_port)]);
    let workload_id = request.workload_id.clone();
    h.workload_start(request).await?;

    let real_addr: std::net::SocketAddr = format!("127.0.0.1:{host_port}").parse()?;
    assert!(table.is_published(Protocol::Tcp, real_addr));

    h.workload_stop(WorkloadStopRequest {
        workload_id: workload_id.clone(),
    })
    .await?;

    assert!(
        !table.is_published(Protocol::Tcp, real_addr),
        "stopping a workload must release its port"
    );
    Ok(())
}

/// A declared port with no `publish` is exactly today's behavior: the service
/// binds it inside the workload and nothing outside can reach it.
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_but_unpublished_port_exposes_nothing() -> Result<()> {
    let table = PortTable::new();
    // Publishing disabled, and the workload must still start.
    let h = start_host(Arc::clone(&table), PublishConfig::default()).await?;
    h.workload_start(service_workload(vec![DeclaredPort {
        name: "echo".into(),
        port: ECHO_VIRTUAL_PORT,
        protocol: Protocol::Tcp,
        publish: None,
        bind: None,
    }]))
    .await?;

    assert!(!table.is_published(
        Protocol::Tcp,
        format!("127.0.0.1:{ECHO_VIRTUAL_PORT}").parse()?
    ));
    Ok(())
}

/// A workload asking to publish on a host that forbids it must fail loudly,
/// not come up silently unexposed.
#[tokio::test(flavor = "multi_thread")]
async fn publishing_without_the_host_flag_fails_the_workload() -> Result<()> {
    let host_port = free_port().await?;
    let h = start_host(PortTable::new(), PublishConfig::default()).await?;
    let response = h
        .workload_start(service_workload(vec![published(host_port)]))
        .await?;
    assert_eq!(
        response.workload_status.workload_state,
        wash_runtime::types::WorkloadState::Error,
        "expected the workload to fail, got: {}",
        response.workload_status.message
    );
    assert!(
        response.workload_status.message.contains("--publish-ports"),
        "got: {}",
        response.workload_status.message
    );
    Ok(())
}

/// A workload and a host component plugin share one port table, so a collision
/// between them is caught rather than producing two listeners that each believe
/// they own the address.
#[tokio::test(flavor = "multi_thread")]
async fn a_workload_cannot_take_a_port_another_owner_holds() -> Result<()> {
    let host_port = free_port().await?;
    let table = PortTable::new();
    let real_addr: std::net::SocketAddr = format!("127.0.0.1:{host_port}").parse()?;

    // Stand in for another owner already holding the port.
    let _held = table.reserve(
        Protocol::Tcp,
        real_addr,
        wash_runtime::host::ports::PortOwner::Plugin("gateway".into()),
    )?;

    let h = start_host(Arc::clone(&table), enabled_config()).await?;
    let response = h
        .workload_start(service_workload(vec![published(host_port)]))
        .await?;
    assert_eq!(
        response.workload_status.workload_state,
        wash_runtime::types::WorkloadState::Error
    );
    assert!(
        response
            .workload_status
            .message
            .contains("host plugin 'gateway'"),
        "the conflict should name the holder, got: {}",
        response.workload_status.message
    );
    Ok(())
}
