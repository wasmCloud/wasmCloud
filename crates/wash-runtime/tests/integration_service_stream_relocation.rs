//! A p3 service's own stream-carrying imports must run on a relocated
//! ephemeral store, as its components' do, so that a backend call never pins
//! the long-lived service store.
//!
//! The values crossing the bridge are the same on either path, which is why
//! `integration_cross_store_bridge` cannot see the difference. This test
//! records trace events and checks which path the call took. It is the only
//! test in its binary because it installs a global subscriber.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::time::timeout;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context as LayerContext, prelude::*, registry::Registry};

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_p3_http_handler};

const BRIDGE_SERVICE_WASM: &[u8] = include_bytes!("wasm/bridge_service.wasm");
const BRIDGE_BACKEND_WASM: &[u8] = include_bytes!("wasm/bridge_backend.wasm");
const STREAM_PRODUCER_P3_WASM: &[u8] = include_bytes!("wasm/stream_producer_p3.wasm");
const STREAM_CONSUMER_P3_WASM: &[u8] = include_bytes!("wasm/stream_consumer_p3.wasm");

const RELOCATED_CALL: &str = "invoking relocated ephemeral dynamic export";

/// Records the message of every event. It has no filter, so it sees trace
/// events whatever the other layers keep.
#[derive(Clone, Default)]
struct MessageRecorder {
    messages: Arc<Mutex<Vec<String>>>,
}

impl<S: Subscriber> Layer<S> for MessageRecorder {
    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        struct Message(Option<String>);
        impl Visit for Message {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = Some(format!("{value:?}"));
                }
            }
        }
        let mut message = Message(None);
        event.record(&mut message);
        if let Some(message) = message.0 {
            self.messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(message);
        }
    }
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

/// A workload with no service, whose HTTP component pulls a stream from another.
fn service_less_stream_workload(host: &str) -> WorkloadStartRequest {
    let component = |name: &str, bytes: &'static [u8]| Component {
        name: name.to_string(),
        digest: None,
        bytes: bytes::Bytes::from_static(bytes),
        local_resources: LocalResources::default(),
        pool_size: 1,
        max_invocations: 100,
        max_concurrency: 1,
        ..Default::default()
    };
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("stream-consumer", STREAM_CONSUMER_P3_WASM),
                component("stream-producer", STREAM_PRODUCER_P3_WASM),
            ],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

#[tokio::test]
async fn service_stream_imports_run_relocated() -> Result<()> {
    let recorder = MessageRecorder::default();
    let messages = Arc::clone(&recorder.messages);
    Registry::default()
        .with(recorder)
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(tracing_subscriber::EnvFilter::from_default_env()),
        )
        .try_init()
        .context("failed to install tracing subscriber")?;

    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    // Without a service, a stream-carrying call stays on the shared store.
    let local = "service-less-stream";
    h.workload_start(service_less_stream_workload(local))
        .await?;
    messages.lock().unwrap().clear();
    let body = timeout(
        Duration::from_secs(15),
        reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .header("HOST", local)
            .send(),
    )
    .await
    .context("request timed out")??
    .text()
    .await?;
    assert_eq!(body, "abcdefghijklmnop", "the stream should cross intact");
    assert!(
        !messages.lock().unwrap().iter().any(|m| m == RELOCATED_CALL),
        "a workload without a service relocated a stream-carrying call"
    );

    let host = "service-stream-relocation";
    h.workload_start(bridge_workload(host)).await?;

    for path in ["/consume", "/produce"] {
        messages.lock().unwrap().clear();
        let resp = timeout(
            Duration::from_secs(15),
            reqwest::Client::new()
                .get(format!("http://{addr}{path}"))
                .header("HOST", host)
                .send(),
        )
        .await
        .context("request timed out")??;
        anyhow::ensure!(resp.status().is_success(), "{path}: {}", resp.status());
        let body = resp.text().await?;
        assert_eq!(
            json_u64_field(&body, "result"),
            json_u64_field(&body, "expected"),
            "{path}: the stream should cross the bridge intact"
        );
        assert!(
            messages.lock().unwrap().iter().any(|m| m == RELOCATED_CALL),
            "{path}: the service's stream-carrying import should run on a relocated store"
        );
    }
    Ok(())
}
