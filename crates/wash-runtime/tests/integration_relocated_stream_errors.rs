//! How a relocated result stream that ends early reaches its reader. A backend
//! that traps after returning the stream fails the reader, as a trap before the
//! reply does. The host's drain limit only ends the stream short, because a trap
//! would stop the reader's whole instance, and logs a warning that names the
//! workload.
//!
//! The drain limit is process-wide and read once, so this binary shortens it for
//! all of its tests, and records warnings with one global subscriber.

#![allow(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::time::timeout;
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context as LayerContext, prelude::*, registry::Registry};

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_p3_http_handler};

const BRIDGE_SERVICE_WASM: &[u8] = include_bytes!("wasm/bridge_service.wasm");
const BRIDGE_BACKEND_WASM: &[u8] = include_bytes!("wasm/bridge_backend.wasm");

const FAILED_AFTER_REPLY: &str = "relocated call failed after returning its results";
const NOT_DRAINED: &str = "did not drain in time";

type Warnings = Arc<Mutex<Vec<String>>>;

/// Records every warning as its fields, `name=value` each.
#[derive(Clone, Default)]
struct WarnRecorder(Warnings);

impl<S: Subscriber> Layer<S> for WarnRecorder {
    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        if *event.metadata().level() != Level::WARN {
            return;
        }
        struct Line(String);
        impl Visit for Line {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                self.0.push_str(&format!(" {}={value:?}", field.name()));
            }
        }
        let mut line = Line(String::new());
        event.record(&mut line);
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.0);
    }
}

/// Shortens the drain limit and starts recording warnings, once per binary.
fn setup() -> Warnings {
    static WARNINGS: OnceLock<Warnings> = OnceLock::new();
    Arc::clone(WARNINGS.get_or_init(|| {
        // SAFETY: every test calls this first, before it starts a host, and
        // `OnceLock` holds the others back until the variable is set.
        unsafe { std::env::set_var("WASH_STREAM_DRAIN_TIMEOUT_SECS", "1") };
        let recorder = WarnRecorder::default();
        let warnings = Arc::clone(&recorder.0);
        tracing::subscriber::set_global_default(Registry::default().with(recorder))
            .expect("no other global subscriber in this binary");
        warnings
    }))
}

fn warned(warnings: &Warnings, workload_id: &str, message: &str) -> bool {
    warnings
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|w| w.contains(message) && w.contains(workload_id))
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

/// Starts the bridge workload and makes one request; returns the workload ID
/// with the answer.
async fn call(host: &str, path: &str) -> Result<(String, Option<(u64, u64)>)> {
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    let request = bridge_workload(host);
    let workload_id = request.workload_id.clone();
    h.workload_start(request).await?;
    Ok((workload_id, get(addr, host, path).await?))
}

#[tokio::test]
async fn a_backend_trap_after_the_reply_fails_the_reader() -> Result<()> {
    let warnings = setup();
    let (workload_id, got) = call("relocated-trap", "/produce-trap").await?;
    assert_eq!(
        got, None,
        "the backend trapped before closing its stream, yet the service read it as finished"
    );
    assert!(
        warned(&warnings, &workload_id, FAILED_AFTER_REPLY),
        "no warning named the workload whose backend trapped"
    );
    Ok(())
}

#[tokio::test]
async fn the_drain_limit_ends_a_stream_short_without_failing_the_reader() -> Result<()> {
    let warnings = setup();
    let (workload_id, got) = call("relocated-late", "/produce-late").await?;
    let (read, expected) = got.expect("a stream ended by the drain limit failed the service");
    assert!(
        read < expected,
        "the stream was not ended by the drain limit: {read} of {expected}"
    );
    assert!(
        warned(&warnings, &workload_id, NOT_DRAINED),
        "no warning named the workload whose stream the drain limit ended"
    );
    Ok(())
}

#[tokio::test]
async fn a_stream_read_in_time_arrives_whole_with_no_warning() -> Result<()> {
    let warnings = setup();
    let (workload_id, got) = call("relocated-in-time", "/produce").await?;
    assert_eq!(got, Some((70_000, 70_000)));
    // Past the drain limit, so a warning for this call would be logged by now.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let logged: Vec<String> = warnings
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|w| w.contains(&workload_id))
        .cloned()
        .collect();
    assert!(logged.is_empty(), "a whole stream logged {logged:?}");
    Ok(())
}
