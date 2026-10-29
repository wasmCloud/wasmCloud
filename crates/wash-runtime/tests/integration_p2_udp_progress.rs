//! A P2 service must survive empty UDP receives between real datagrams.
use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use wash_runtime::{
    engine::Engine,
    host::allowed_loopback::AllowedLoopbackPort,
    host::{HostApi, HostBuilder},
    sockets::{internal_names::HOST_SENTINEL, policy::SocketPolicy},
    types::{LocalResources, Service, Workload, WorkloadStartRequest, WorkloadStopRequest},
};

const GUEST: &[u8] = include_bytes!("wasm/p2_udp_progress.wasm");

#[tokio::test]
async fn p2_service_udp_roundtrips_survive_empty_receive_polls() -> Result<()> {
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let port = peer.local_addr()?.port();
    let engine = Engine::builder()
        .with_pooling_allocator(false)
        .with_socket_policy(Arc::new(SocketPolicy {
            host_loopback_enabled: true,
            ..Default::default()
        }))
        .build()?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .build()?
        .start()
        .await?;
    let id = uuid::Uuid::new_v4().to_string();
    let start = host
        .workload_start(WorkloadStartRequest {
            workload_id: id.clone(),
            workload: Workload {
                namespace: "test".into(),
                name: "p2-udp-progress".into(),
                annotations: HashMap::new(),
                components: vec![],
                host_interfaces: vec![],
                volumes: vec![],
                service: Some(Service {
                    digest: None,
                    bytes: bytes::Bytes::from_static(GUEST),
                    max_restarts: 0,
                    local_resources: LocalResources {
                        environment: HashMap::from([(
                            "WASH_UDP_PROGRESS_PEER".into(),
                            format!("{HOST_SENTINEL}:{port}"),
                        )]),
                        allowed_host_loopback_ports: Arc::from([AllowedLoopbackPort::udp(port)]),
                        ..Default::default()
                    },
                }),
            },
        })
        .await;
    let observed = async {
        start.context("start P2 UDP progress service")?;
        for seq in 1..=3 {
            let mut buffer = [0u8; 128];
            let (n, address) = peer.recv_from(&mut buffer).await?;
            assert_eq!(&buffer[..n], format!("udp-progress-{seq}").as_bytes());
            peer.send_to(&buffer[..n], address).await?;
        }
        let mut buffer = [0u8; 128];
        let (n, _) = peer.recv_from(&mut buffer).await?;
        assert_eq!(&buffer[..n], b"udp-progress-complete");
        Ok::<_, anyhow::Error>(())
    };
    let outcome = tokio::time::timeout(Duration::from_secs(10), observed).await;
    host.workload_stop(WorkloadStopRequest { workload_id: id })
        .await?;
    outcome??;
    Ok(())
}
