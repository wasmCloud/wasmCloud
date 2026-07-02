//! Spike: the Reactor co-drives a p2 `wasmcloud:messaging/handler` alongside a
//! p3 `wasi:cli/run` on one long-lived instance.
//!
//! The `msg-counter` fixture exports BOTH `wasi:cli/run@0.3` and the p2
//! `wasmcloud:messaging/handler@0.2.0`. Its `handle-message` increments a
//! process-global counter and echoes `"{count}:{subject}"`. Delivering two
//! messages and observing the count climb (1, then 2) proves the p2 handler runs
//! on the SAME long-lived instance the reactor co-drives — invoked via the
//! dynamic `call_concurrent` path under `run_concurrent` — rather than a fresh
//! instance per message.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::http::{DevRouter, HostHandler, HttpServer};
use wash_runtime::host::reactor::BrokerMessage;
use wash_runtime::host::HostApi;
use wash_runtime::types::{LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::start_host_with_p3_handler;

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
