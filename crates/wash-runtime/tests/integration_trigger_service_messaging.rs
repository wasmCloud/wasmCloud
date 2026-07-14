//! Spike: the TriggerService co-drives a p2 `wasmcloud:messaging/handler` alongside a
//! p3 `wasi:cli/run` on one long-lived instance.
//!
//! The `msg-counter` fixture exports BOTH `wasi:cli/run@0.3` and the p2
//! `wasmcloud:messaging/handler@0.2.0`. Its `handle-message` increments a
//! process-global counter and echoes `"{count}:{subject}"`. Delivering two
//! messages and observing the count climb (1, then 2) proves the p2 handler runs
//! on the SAME long-lived instance the trigger service co-drives — invoked via the
//! dynamic `call_concurrent` path under `run_concurrent` — rather than a fresh
//! instance per message.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::host::http::{DevRouter, HostHandler, HttpServer};
use wash_runtime::host::trigger_service::BrokerMessage;
use wash_runtime::types::{
    LocalResources, Service, Workload, WorkloadStartRequest, WorkloadStopRequest,
};

mod common;
use common::start_host_with_p3_handler;

const MSG_COUNTER_WASM: &[u8] = include_bytes!("wasm/msg_counter.wasm");

fn msg_counter_request(workload_id: &str, max_restarts: u64) -> WorkloadStartRequest {
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
                max_restarts,
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
        http_server.deliver_trigger_service_message(workload_id, msg),
    )
    .await
    .context("deliver_trigger_service_message timed out")?
}

#[tokio::test]
async fn test_trigger_service_co_drives_messaging_handler() -> Result<()> {
    let workload_id = uuid::Uuid::new_v4().to_string();
    let (addr, host, http_server) = start_host_with_p3_handler("127.0.0.1:0").await?;

    host.workload_start(msg_counter_request(&workload_id, 0))
        .await
        .context("failed to start msg-counter trigger service workload")?;

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

    // Multi-ingress: the same instance also serves HTTP (`wasi:http/handler`
    // is a second co-driven ingress), and its response reads the SAME
    // process-global count the messaging handler advanced — proving both
    // ingresses share one live instance.
    let client = reqwest::Client::new();
    let resp = timeout(
        Duration::from_secs(10),
        client.get(format!("http://{addr}/")).send(),
    )
    .await
    .context("HTTP request to the co-driven instance timed out")??;
    anyhow::ensure!(
        resp.status().is_success(),
        "the co-driven instance should serve HTTP, got {}",
        resp.status()
    );
    let body = resp.text().await?;
    assert_eq!(
        common::json_u64_field(&body, "count"),
        2,
        "the HTTP ingress must observe the messaging ingress's state (one shared \
         instance), got {body}"
    );

    Ok(())
}

/// Teardown: a stopped trigger service drops its messaging subscription, so a
/// message published afterward is not delivered. Guards the unbind wiring — a
/// stopped service must deregister its handler, not keep receiving deliveries on
/// a torn-down instance.
#[tokio::test]
async fn test_trigger_service_stop_drops_messaging_subscription() -> Result<()> {
    let workload_id = uuid::Uuid::new_v4().to_string();
    let (_addr, host, http_server) = start_host_with_p3_handler("127.0.0.1:0").await?;

    host.workload_start(msg_counter_request(&workload_id, 0))
        .await
        .context("failed to start msg-counter trigger service workload")?;

    // Delivery works while running.
    let live = deliver(&http_server, &workload_id, "live").await?;
    assert_eq!(
        live,
        Err("1:live".to_string()),
        "message handled while running"
    );

    host.workload_stop(WorkloadStopRequest {
        workload_id: workload_id.clone(),
    })
    .await
    .context("failed to stop trigger service workload")?;

    // Stop must DEREGISTER the handler, not merely tear down the instance: the
    // registration is gone, so delivery fails at lookup rather than reaching a
    // torn-down channel.
    assert!(
        !http_server
            .has_trigger_service_messaging(&workload_id)
            .await,
        "stop must drop the messaging subscription"
    );
    let after = deliver(&http_server, &workload_id, "after").await;
    assert!(
        after.is_err(),
        "a message published after stop must not be delivered, got {after:?}"
    );

    Ok(())
}

/// Restart: a handler trap faults the co-driven instance; the supervisor restarts
/// it within `max_restarts` and re-registers the messaging handler, so delivery
/// resumes on a FRESH instance (its per-instance count reset to 1, not 2).
///
/// A trap leaves the shared instance unenterable for every later message, so
/// [`MessagingTask`] reports it and then faults the driver out of
/// `run_concurrent`, letting the workload supervisor re-instantiate and
/// re-register. An ordinary handler `Err(string)` outcome does NOT fault the
/// driver — the co-drive test above proves consecutive deliveries keep the
/// instance.
///
/// [`MessagingTask`]: wash_runtime::host::trigger_service
#[tokio::test]
async fn test_trigger_service_restarts_and_resubscribes_on_fault() -> Result<()> {
    let workload_id = uuid::Uuid::new_v4().to_string();
    let (_addr, host, http_server) = start_host_with_p3_handler("127.0.0.1:0").await?;
    host.workload_start(msg_counter_request(&workload_id, 3))
        .await
        .context("failed to start msg-counter trigger service workload")?;

    // Prime the count on the initial instance.
    let r1 = deliver(&http_server, &workload_id, "first").await?;
    assert_eq!(
        r1,
        Err("1:first".to_string()),
        "handled on the initial instance"
    );

    // `boom` traps the handler, faulting the instance; the supervisor restarts it.
    let _ = deliver(&http_server, &workload_id, "boom").await;

    // After the restart, delivery resumes (re-subscribed) on a fresh instance
    // whose per-instance count starts over at 1. The restart is async, so poll
    // until a delivery is HANDLED again — a `{count}:{subject}` echo. A delivery
    // that races the teardown of the poisoned incarnation surfaces a trap error
    // string instead ("cannot enter component instance"); keep polling through
    // those rather than mistaking them for a handled message.
    let mut got = None;
    for _ in 0..100 {
        if let Ok(Err(s)) = deliver(&http_server, &workload_id, "after").await
            && s.ends_with(":after")
        {
            got = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        got,
        Some("1:after".to_string()),
        "after a fault the supervisor re-subscribes on a fresh instance (count reset)"
    );

    Ok(())
}
