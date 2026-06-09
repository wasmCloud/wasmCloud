//! Integration tests for wasi:tls round-trip (P3 only).
//!
//! A P3 guest component opens a `wasi:sockets` TCP stream, wraps it with
//! `wasi:tls/client::Connector`, and round-trips a `PING\r\n` / `PONG\r\n`
//! exchange against a local rustls echo server started in-process.

#![cfg(feature = "wasi-tls")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::{HostApi, HostBuilder},
    types::{
        LocalResources, Service, SocketTunnelMode, SocketTunnelPolicy, Workload,
        WorkloadStartRequest, WorkloadState,
    },
};

mod common;
use common::tls::{
    EchoServer, engine_with_p3_and_tls, install_default_crypto_provider, start_tls_echo_server,
};

const TLS_ECHO_CLIENT_P3_WASM: &[u8] = include_bytes!("wasm/tls_echo_client_p3.wasm");

fn echo_client_workload_request(
    name: &str,
    wasm: &'static [u8],
    echo_addr: std::net::SocketAddr,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: name.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(wasm),
                local_resources: LocalResources {
                    memory_limit_mb: 256,
                    cpu_limit: 1,
                    environment: HashMap::from([("ECHO_ADDR".to_string(), echo_addr.to_string())]),
                    // The echo server lives on a non-loopback interface, which the
                    // default strict policy blocks. This test exercises TLS-over-TCP,
                    // not the tunnel policy, so opt into allow-all to let the guest
                    // dial the server directly.
                    socket_tunnels: Some(SocketTunnelPolicy {
                        mode: SocketTunnelMode::AllowAll,
                        rules: HashMap::new(),
                    }),
                    ..Default::default()
                },
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    }
}

/// `test_p3_tls_tcp_round_trip` exercises:
///
/// - TCP connect to a non-loopback echo server (loopback is intercepted by the runtime)
/// - TLS handshake against a self-signed cert via the custom `TestTlsProvider`
/// - Plaintext `PING\r\n` write through the encrypted stream
/// - Receipt verified via oneshot channel on the server side
#[cfg(feature = "wasip3")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_p3_tls_tcp_round_trip() -> Result<()> {
    install_default_crypto_provider();

    let EchoServer {
        addr: echo_addr,
        cert_der,
        ping_rx,
    } = start_tls_echo_server().await?;

    let engine = engine_with_p3_and_tls(&cert_der)?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .build()?
        .start()
        .await?;

    let req =
        echo_client_workload_request("tls-echo-client-p3", TLS_ECHO_CLIENT_P3_WASM, echo_addr);

    let resp = timeout(Duration::from_secs(15), host.workload_start(req))
        .await
        .context("P3 tls round-trip test timed out")?
        .context("workload_start failed")?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "expected Running, got {:?} (message: {})",
        resp.workload_status.workload_state,
        resp.workload_status.message,
    );

    let ping = timeout(Duration::from_secs(10), ping_rx)
        .await
        .context("echo server did not receive PING within timeout")?
        .context("echo server task dropped its sender before sending PING")?;
    assert!(
        ping.starts_with(b"PING\r\n"),
        "echo server received unexpected bytes from guest: {ping:?}",
    );

    Ok(())
}
