//! Integration tests for wasi:sockets + wasi:tls round-trip.
//!
//! # P2 round-trip
//! A P2 guest component opens a `wasi:sockets` TCP stream, wraps it with
//! `wasi:tls/types/client-handshake`, and round-trips a `PING\r\n` /
//! `PONG\r\n` exchange against a local rustls echo server started in-process.
//!
//! # P3 round-trip
//! Same scenario, but using P3 streams and `wasi:tls/client::Connector`.

#![cfg(feature = "wasi-tls")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::{HostApi, HostBuilder},
    types::{LocalResources, Service, Workload, WorkloadStartRequest},
};

mod common;
use common::tls::{engine_with_tls, install_default_crypto_provider, start_tls_echo_server};

#[cfg(feature = "wasip3")]
use common::tls::engine_with_p3_and_tls;

const TLS_ECHO_CLIENT_P2_WASM: &[u8] = include_bytes!("wasm/tls_echo_client_p2.wasm");

#[cfg(feature = "wasip3")]
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
                    config: HashMap::new(),
                    environment: HashMap::from([("ECHO_ADDR".to_string(), echo_addr.to_string())]),
                    volume_mounts: vec![],
                    allowed_hosts: Default::default(),
                },
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    }
}

/// The tls-echo-client-p2 component opens a TCP connection, performs a TLS
/// handshake, sends `PING\r\n`, and expects `PONG\r\n` back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_p2_tls_tcp_round_trip() -> Result<()> {
    install_default_crypto_provider();

    let (echo_addr, cert_der) = start_tls_echo_server().await?;

    let engine = engine_with_tls(&cert_der)?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .build()?
        .start()
        .await?;

    let req =
        echo_client_workload_request("tls-echo-client-p2", TLS_ECHO_CLIENT_P2_WASM, echo_addr);

    timeout(Duration::from_secs(15), host.workload_start(req))
        .await
        .context("tls round-trip test timed out")?
        .context("workload_start failed")?;

    tokio::time::sleep(Duration::from_secs(3)).await;

    Ok(())
}

/// The tls-echo-client-p3 component does the same round-trip using P3 streams
/// and `wasi:tls/client::Connector`.
#[cfg(feature = "wasip3")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_p3_tls_tcp_round_trip() -> Result<()> {
    install_default_crypto_provider();

    let (echo_addr, cert_der) = start_tls_echo_server().await?;

    let engine = engine_with_p3_and_tls(&cert_der)?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .build()?
        .start()
        .await?;

    let req =
        echo_client_workload_request("tls-echo-client-p3", TLS_ECHO_CLIENT_P3_WASM, echo_addr);

    timeout(Duration::from_secs(15), host.workload_start(req))
        .await
        .context("P3 tls round-trip test timed out")?
        .context("workload_start failed")?;

    tokio::time::sleep(Duration::from_secs(3)).await;

    Ok(())
}
