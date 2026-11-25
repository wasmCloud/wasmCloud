//! A service that listens on a loopback port and traps binds the same port
//! again after the supervisor restarts it, for p2 and p3 services.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use tokio::time::{Instant, timeout};

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const SVC_TCP_ECHO_P2_WASM: &[u8] = include_bytes!("wasm/svc_tcp_echo_p2.wasm");
const SVC_TCP_ECHO_P3_WASM: &[u8] = include_bytes!("wasm/svc_tcp_echo.wasm");
const HTTP_LOOPBACK_GATEWAY_WASM: &[u8] = include_bytes!("wasm/http_loopback_gateway.wasm");

/// How long the service gets to answer on its port, at start and after the
/// restart.
const ANSWER_WITHIN: Duration = Duration::from_secs(15);

/// One gateway request, which the gateway relays to 127.0.0.1:9099.
async fn get(
    client: &reqwest::Client,
    addr: SocketAddr,
    host: &str,
    path: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let response = timeout(
        Duration::from_secs(2),
        client
            .get(format!("http://{addr}{path}"))
            .header("HOST", host)
            .send(),
    )
    .await
    .context("the gateway request timed out")??;
    let status = response.status();
    Ok((status, response.text().await?))
}

/// Poll the gateway until the service answers on its loopback port.
async fn wait_for_echo(client: &reqwest::Client, addr: SocketAddr, host: &str) -> Result<()> {
    let deadline = Instant::now() + ANSWER_WITHIN;
    loop {
        let last = match get(client, addr, host, "/").await {
            Ok((status, body)) if status.is_success() && body == "echo:ping" => return Ok(()),
            Ok((status, body)) => format!("{status}: {body}"),
            Err(error) => format!("{error:#}"),
        };
        if Instant::now() >= deadline {
            bail!("the service did not answer on 127.0.0.1:9099; last response: {last}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Start `service` behind the gateway, trap it, and check that its restarted
/// incarnation listens on the same loopback port.
async fn rebinds_its_port_after_a_trap(service: &'static [u8], host: &str) -> Result<()> {
    let (addr, runtime) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    runtime
        .workload_start(WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "test".to_string(),
                name: host.to_string(),
                annotations: HashMap::new(),
                service: Some(Service {
                    digest: None,
                    bytes: bytes::Bytes::from_static(service),
                    local_resources: LocalResources::default(),
                    max_restarts: 2,
                }),
                components: vec![Component {
                    name: "http-loopback-gateway".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(HTTP_LOOPBACK_GATEWAY_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 1000,
                    max_concurrency: 1,
                    ..Default::default()
                }],
                host_interfaces: http_only_host_interfaces(host),
                volumes: vec![],
            },
        })
        .await
        .context("failed to start the echo workload")?;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;

    wait_for_echo(&client, addr, host)
        .await
        .context("before the trap")?;
    let (status, body) = get(&client, addr, host, "/trap").await?;
    ensure!(
        !status.is_success(),
        "the service answered a trap request instead of trapping: {body}"
    );
    wait_for_echo(&client, addr, host)
        .await
        .context("after the restart")
}

#[tokio::test]
async fn p2_service_binds_its_loopback_port_again_after_a_trap() -> Result<()> {
    rebinds_its_port_after_a_trap(SVC_TCP_ECHO_P2_WASM, "loopback-restart-p2").await
}

#[tokio::test]
async fn p3_service_binds_its_loopback_port_again_after_a_trap() -> Result<()> {
    rebinds_its_port_after_a_trap(SVC_TCP_ECHO_P3_WASM, "loopback-restart-p3").await
}
