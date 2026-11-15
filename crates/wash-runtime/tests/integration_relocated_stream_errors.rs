//! A relocated result stream that ends early, because its backend trapped after
//! returning it or because the host's drain limit cut it off, must reach the
//! reader as an error, not as a stream that finished.
//!
//! The drain limit is process-wide and read once, so this binary shortens it for
//! all of its tests.

#![allow(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, net::SocketAddr, time::Duration};

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_p3_http_handler};

const BRIDGE_SERVICE_WASM: &[u8] = include_bytes!("wasm/bridge_service.wasm");
const BRIDGE_BACKEND_WASM: &[u8] = include_bytes!("wasm/bridge_backend.wasm");

fn short_drain_limit() {
    static SET: std::sync::Once = std::sync::Once::new();
    // SAFETY: every test calls this first, before it starts a host, and `Once`
    // holds the others back until the variable is set.
    SET.call_once(|| unsafe { std::env::set_var("WASH_STREAM_DRAIN_TIMEOUT_SECS", "1") });
}

fn bridge_workload(host: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(BRIDGE_SERVICE_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            components: vec![Component {
                name: "bridge-backend".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(BRIDGE_BACKEND_WASM),
                local_resources: LocalResources::default(),
                pool_size: 0,
                max_invocations: 0,
                max_concurrency: 1,
                ..Default::default()
            }],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

/// The `(result, expected)` pair from a 2xx answer, or `None` when the exchange
/// failed, which is how a trapped service reaches the client.
async fn get(addr: SocketAddr, host: &str, path: &str) -> Result<Option<(u64, u64)>> {
    let sent = timeout(
        Duration::from_secs(30),
        reqwest::Client::new()
            .get(format!("http://{addr}{path}"))
            .header("HOST", host)
            .send(),
    )
    .await
    .with_context(|| format!("{path}: no answer in 30 s"))?;
    let Ok(response) = sent else {
        return Ok(None);
    };
    if !response.status().is_success() {
        return Ok(None);
    }
    let Ok(body) = response.text().await else {
        return Ok(None);
    };
    Ok(Some((
        json_u64_field(&body, "result"),
        json_u64_field(&body, "expected"),
    )))
}

async fn call(host: &str, path: &str) -> Result<Option<(u64, u64)>> {
    short_drain_limit();
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    h.workload_start(bridge_workload(host)).await?;
    get(addr, host, path).await
}

#[tokio::test]
async fn a_backend_trap_after_the_reply_is_not_a_finished_stream() -> Result<()> {
    let got = call("relocated-trap", "/produce-trap").await?;
    assert_eq!(
        got, None,
        "the backend trapped before closing its stream, yet the service read it as finished"
    );
    Ok(())
}

#[tokio::test]
async fn a_stream_cut_by_the_drain_limit_is_not_a_finished_stream() -> Result<()> {
    let got = call("relocated-late", "/produce-late").await?;
    assert_eq!(
        got, None,
        "the drain limit cut the stream, yet the service read it as finished"
    );
    Ok(())
}

#[tokio::test]
async fn a_stream_read_in_time_still_arrives_whole() -> Result<()> {
    let got = call("relocated-in-time", "/produce").await?;
    assert_eq!(got, Some((70_000, 70_000)));
    Ok(())
}
