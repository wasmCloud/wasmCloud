//! Service with `wasmcloud:messaging/handler` alongside a
//! p3 `wasi:cli/run` on one long-lived instance.
//!
//! The `msg-counter` fixture exports BOTH `wasi:cli/run@0.3` and the p2
//! `wasmcloud:messaging/handler@0.2.0`. Its `handle-message` increments a
//! process-global counter and echoes `"{count}:{subject}"`. Delivering two
//! messages and observing the count climb (1, then 2) proves the p2 handler runs
//! on the SAME long-lived instance, invoked via the
//! dynamic `call_concurrent` path under `run_concurrent`, rather than a fresh
//! instance per message.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, HostHandler, HttpServer};
use wash_runtime::host::reactor::BrokerMessage;
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::wasmcloud_messaging::InMemoryMessaging;
use wash_runtime::plugin::{
    wasi_blobstore::InMemoryBlobstore, wasi_config::DynamicConfig,
    wasi_keyvalue::InMemoryKeyValue, wasi_logging::TracingLogger,
};
use wash_runtime::types::{LocalResources, Service, Workload, WorkloadStartRequest};
use wash_runtime::wit::WitInterface;

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_handler};

const MSG_COUNTER_WASM: &[u8] = include_bytes!("wasm/msg_counter.wasm");

fn msg_counter_request(workload_id: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "msg-counter".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(MSG_COUNTER_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    }
}

async fn deliver(
    http_server: &Arc<HttpServer<DevRouter>>,
    workload_id: &str,
    subject: &str,
) -> Result<Result<(), String>> {
    let msg = BrokerMessage {
        subject: subject.to_string(),
        body: b"hi".to_vec(),
        reply_to: None,
    };
    timeout(
        Duration::from_secs(10),
        http_server.deliver_reactor_message(workload_id, msg),
    )
    .await
    .context("deliver_reactor_message timed out")?
}

#[tokio::test]
async fn test_reactor_co_drives_messaging_handler() -> Result<()> {
    let workload_id = uuid::Uuid::new_v4().to_string();
    let (_addr, host, http_server) = start_host_with_p3_handler("127.0.0.1:0").await?;

    host.workload_start(msg_counter_request(&workload_id))
        .await
        .context("failed to start msg-counter reactor workload")?;

    // Two messages land on the SAME long-lived instance, so the handler's
    // process-global count climbs 1 -> 2 (a fresh instance per message would
    // return 1 both times).
    let r1 = deliver(&http_server, &workload_id, "first").await?;
    assert_eq!(
        r1,
        Err("1:first".to_string()),
        "first message handled on the co-driven instance"
    );

    let r2 = deliver(&http_server, &workload_id, "second").await?;
    assert_eq!(
        r2,
        Err("2:second".to_string()),
        "second message hits the same long-lived instance (count persists)"
    );

    Ok(())
}

/// The `wasmcloud:messaging/handler` host interface, so the messaging plugin
/// binds to the service (empty config → the service subscribes to everything).
fn messaging_handler_interface() -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "messaging".to_string(),
        interfaces: ["handler".to_string()].into_iter().collect(),
        version: Some(semver::Version::new(0, 2, 0)),
        config: HashMap::new(),
        name: None,
    }
}

fn msg_counter_e2e_request(workload_id: &str, host: &str) -> WorkloadStartRequest {
    let mut host_interfaces = http_only_host_interfaces(host);
    host_interfaces.push(messaging_handler_interface());
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(MSG_COUNTER_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces,
            volumes: vec![],
        },
    }
}

/// Read the reactor's `{"count":N}` over HTTP.
async fn get_count(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host: &str,
) -> Result<u64> {
    let resp = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/count"))
            .header("HOST", host)
            .send(),
    )
    .await
    .context("GET /count timed out")??;
    anyhow::ensure!(resp.status().is_success(), "GET /count: {}", resp.status());
    let body = resp.text().await?;
    body.split("\"count\":")
        .nth(1)
        .and_then(|rest| rest.trim_end_matches('}').trim().parse().ok())
        .context("failed to parse count")
}

/// End-to-end: a message published through the in-memory messaging backend is
/// delivered to the long-lived reactor (not a fresh per-message instance), so
/// the count observed over the reactor's HTTP handler, advances.
#[tokio::test]
async fn test_reactor_messaging_via_in_memory_backend() -> Result<()> {
    let engine = Engine::builder().build()?;
    let http_server = Arc::new(HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?);
    let addr = http_server.addr();
    let messaging = Arc::new(InMemoryMessaging::new());
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(http_server.clone())
        .with_plugin(Arc::new(InMemoryBlobstore::new(None)))?
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .with_plugin(messaging.clone())?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let host_header = "msg-e2e";
    host.workload_start(msg_counter_e2e_request(&workload_id, host_header))
        .await
        .context("failed to start msg-counter reactor workload")?;

    let client = reqwest::Client::new();
    assert_eq!(
        get_count(&client, addr, host_header).await?,
        0,
        "no messages handled yet"
    );

    // Publish through the in-memory backend; it routes to the subscribed reactor
    // service, whose receive loop delivers to the co-driven instance.
    messaging
        .publish(&workload_id, "test.subject", b"hello".to_vec())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?;

    // Delivery + handling is async; poll until the count reflects it.
    let mut observed = 0;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        observed = get_count(&client, addr, host_header).await?;
        if observed >= 1 {
            break;
        }
    }
    assert_eq!(
        observed, 1,
        "the published message was handled on the co-driven reactor instance"
    );

    Ok(())
}
