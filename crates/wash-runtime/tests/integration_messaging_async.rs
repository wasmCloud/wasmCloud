//! End-to-end coverage for the async `wasmcloud:messaging@0.3.0` surface.
//!
//! The `messaging-echo-p3` fixture exports `wasmcloud:messaging/handler@0.3.0`
//! as an `async func` and, from inside it, `await`s the imported
//! `consumer.publish@0.3.0`. It bumps its message counter only AFTER that
//! publish resolves, so an observed count is evidence of the whole async path:
//! the host invoked an async-lifted export through the concurrent ABI, and the
//! guest awaited an async import to completion while inside it.
//!
//! The fixture is a service, so delivery goes through the trigger service's
//! `handle-message` lookup and result lifting — the `@0.3.0` half of which these
//! tests are the end-to-end cover for.
//!
//! These use the in-memory backend, so they need no Docker and run in the
//! default CI leg.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::wasmcloud_messaging::InMemoryMessaging;
use wash_runtime::plugin::{
    wasi_blobstore::InMemoryBlobstore, wasi_config::DynamicConfig, wasi_keyvalue::InMemoryKeyValue,
    wasi_logging::TracingLogger,
};
use wash_runtime::types::{
    Component, LocalResources, Service, Workload, WorkloadStartRequest, WorkloadState,
};
use wash_runtime::wit::WitInterface;

mod common;
use common::{http_only_host_interfaces, json_u64_field};

const ECHO_P3_WASM: &[u8] = include_bytes!("wasm/messaging_echo_p3.wasm");
const SYNC_ECHO_WASM: &[u8] = include_bytes!("wasm/messaging_echo.wasm");

/// A `wasmcloud:messaging` host-interface entry at the given version.
///
/// The version is what selects the surface: `0.3.0` routes the workload to the
/// async bindings, `0.2.0` to the sync ones.
fn messaging_interface(version: (u64, u64, u64), subscriptions: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "messaging".to_string(),
        interfaces: ["handler".to_string(), "consumer".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::new(version.0, version.1, version.2)),
        config: HashMap::from([("subscriptions".to_string(), subscriptions.to_string())]),
        name: None,
    }
}

/// Start the async fixture as a *service*, so the trigger service co-drives one
/// long-lived instance and its `MSG_COUNT` survives between messages.
fn async_echo_request(workload_id: &str, host_header: &str, subject: &str) -> WorkloadStartRequest {
    let mut host_interfaces = http_only_host_interfaces(host_header);
    host_interfaces.push(messaging_interface((0, 3, 0), subject));
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host_header.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(ECHO_P3_WASM),
                local_resources: LocalResources {
                    config: HashMap::from([("subscriptions".to_string(), subject.to_string())]),
                    ..Default::default()
                },
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces,
            volumes: vec![],
        },
    }
}

/// Start the sync `@0.2.0` echo fixture as a per-message component. Used only to
/// prove the two surfaces bind alongside each other.
fn sync_echo_request(workload_id: &str, host_header: &str, subject: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host_header.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "echo".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(SYNC_ECHO_WASM),
                local_resources: LocalResources {
                    config: HashMap::from([("subscriptions".to_string(), subject.to_string())]),
                    ..Default::default()
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: vec![messaging_interface((0, 2, 0), subject)],
            volumes: vec![],
        },
    }
}

struct Harness<H: HostApi> {
    host: Arc<H>,
    addr: std::net::SocketAddr,
    messaging: Arc<InMemoryMessaging>,
    client: reqwest::Client,
}

async fn start_harness() -> Result<Harness<impl HostApi>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let messaging = Arc::new(InMemoryMessaging::new());
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(InMemoryBlobstore::new(None)))?
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .with_plugin(messaging.clone())?
        .build()?;
    let host = host.start().await.context("failed to start host")?;
    Ok(Harness {
        host,
        addr,
        messaging,
        client: reqwest::Client::new(),
    })
}

impl<H: HostApi> Harness<H> {
    /// Read the fixture's `{"count":N}` over its HTTP handler.
    async fn count(&self, host_header: &str) -> Result<u64> {
        let resp = timeout(
            Duration::from_secs(10),
            self.client
                .get(format!("http://{}/count", self.addr))
                .header("HOST", host_header)
                .send(),
        )
        .await
        .context("GET /count timed out")??;
        anyhow::ensure!(resp.status().is_success(), "GET /count: {}", resp.status());
        Ok(json_u64_field(&resp.text().await?, "count"))
    }

    /// Poll until the handled-message count reaches `want`, or give up.
    async fn await_count(&self, host_header: &str, want: u64) -> Result<u64> {
        let mut observed = 0;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            observed = self.count(host_header).await?;
            if observed >= want {
                break;
            }
        }
        Ok(observed)
    }
}

/// The headline case: a message published through the backend reaches an
/// `async fn handle_message` in a real guest, which awaits an async
/// `consumer.publish` to reply before returning.
#[tokio::test]
async fn async_handler_receives_and_replies() -> Result<()> {
    let h = start_harness().await?;
    let workload_id = uuid::Uuid::new_v4().to_string();
    let host_header = "async-echo";

    let state = h
        .host
        .workload_start(async_echo_request(&workload_id, host_header, "test.async"))
        .await
        .context("failed to start the async echo workload")?;
    assert_eq!(
        state.workload_status.workload_state,
        WorkloadState::Running,
        "workload should be running: {:?}",
        state.workload_status.message
    );

    assert_eq!(h.count(host_header).await?, 0, "no messages handled yet");

    // `reply_to` is set, so the guest's handler must await an async
    // `consumer.publish` before it returns — the reply path is what would break
    // if the async import were mis-bound.
    h.messaging
        .publish(&workload_id, "test.async", b"hello".to_vec())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?;

    assert_eq!(
        h.await_count(host_header, 1).await?,
        1,
        "the async handler should have been invoked exactly once"
    );
    Ok(())
}

/// Several messages in flight: the async handler is re-entered per message and
/// every one is accounted for. Under `@0.2.0` each `publish` reply blocked the
/// instance; here they are awaited.
#[tokio::test]
async fn async_handler_handles_messages_concurrently() -> Result<()> {
    let h = start_harness().await?;
    let workload_id = uuid::Uuid::new_v4().to_string();
    let host_header = "async-echo-many";

    h.host
        .workload_start(async_echo_request(&workload_id, host_header, "test.many"))
        .await
        .context("failed to start the async echo workload")?;

    const N: u64 = 5;
    for i in 0..N {
        h.messaging
            .publish(&workload_id, "test.many", format!("msg-{i}").into_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("publish {i} failed: {e}"))?;
    }

    assert_eq!(
        h.await_count(host_header, N).await?,
        N,
        "every published message should reach the async handler"
    );
    Ok(())
}

/// The regression that matters most: a `@0.2.0` guest and a `@0.3.0` guest on
/// ONE host. The two surfaces are separate linker instances served by the same
/// plugin, so binding one must not disturb the other.
#[tokio::test]
async fn sync_and_async_guests_coexist_on_one_host() -> Result<()> {
    let h = start_harness().await?;

    let async_id = uuid::Uuid::new_v4().to_string();
    h.host
        .workload_start(async_echo_request(
            &async_id,
            "coexist-async",
            "test.coexist",
        ))
        .await
        .context("failed to start the async workload")?;

    // The sync fixture exports no HTTP handler, so it only has to bind and run;
    // that it does so alongside the async one is the point.
    let sync_id = uuid::Uuid::new_v4().to_string();
    let state = h
        .host
        .workload_start(sync_echo_request(&sync_id, "coexist-sync", "test.coexist"))
        .await
        .context("failed to start the sync workload alongside the async one")?;
    assert_eq!(
        state.workload_status.workload_state,
        WorkloadState::Running,
        "the @0.2.0 workload must still bind with an @0.3.0 workload on the host"
    );

    // The async workload still works with the sync one bound.
    h.messaging
        .publish(&async_id, "test.coexist", b"hello".to_vec())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?;
    assert_eq!(
        h.await_count("coexist-async", 1).await?,
        1,
        "the async handler should still receive messages"
    );

    // Messaging is per-workload isolated, so the sync workload saw nothing from
    // the async workload's publish.
    Ok(())
}
