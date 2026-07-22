//! End-to-end tests for the washlet NATS API — the wire surface the
//! runtime-operator drives (`runtime.host.{id}.*` requests, JSON-encoded v2
//! types, and `runtime.operator.heartbeat.{id}` publishes).
//!
//! Pins the contracts the operator's controllers depend on:
//! - `reconcilePlacement`'s retry loop: re-sending `workload.start` for a
//!   live workload ID is rejected with `WORKLOAD_STATE_ERROR`, echoes the
//!   workload ID (the operator records it from the response without checking
//!   the state), and leaves the original workload untouched; the ID becomes
//!   reusable only after an explicit `workload.stop`.
//! - A start that fails before the host reserves the ID (OCI pull failure in
//!   the washlet) does not consume the ID.
//! - `finalize`'s idempotent teardown: stop/status of an unknown ID answer
//!   `WORKLOAD_STATE_NOT_FOUND` instead of erroring.
//! - Host registration: the published heartbeat and the `heartbeat` RPC carry
//!   the host ID, the `hostgroup` label, and the environment the operator
//!   records verbatim, plus a workload count that tracks running workloads.
//!
//! Requires Docker (NATS); marked `#[ignore]`, run with `cargo test --include-ignored`.

#![cfg(feature = "washlet")]

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use testcontainers::{
    ContainerAsync, GenericImage,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use wash_runtime::washlet::{ClusterHostBuilder, heartbeat_subject, rpc_subject, types::v2};

const HOST_GROUP: &str = "e2e";
const ENVIRONMENT: &str = "e2e-env";

struct TestHarness {
    api_client: async_nats::Client,
    host_id: String,
    /// Subscribed before the host starts, so it observes the heartbeat the
    /// washlet publishes on its immediate first tick.
    heartbeat_sub: async_nats::Subscriber,
    shutdown: Pin<Box<dyn Future<Output = Result<()>> + Send>>,
    _container: ContainerAsync<GenericImage>,
}

async fn setup() -> Result<TestHarness> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let container = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start NATS container: {e}"))?;
    let port = container
        .get_host_port_ipv4(4222)
        .await
        .map_err(|e| anyhow::anyhow!("failed to get NATS host port: {e}"))?;
    let nats_url = format!("nats://127.0.0.1:{port}");

    // The washlet holds its own client; the tests drive the API over a
    // separate connection, mirroring the operator being a distinct peer.
    let washlet_client = Arc::new(
        async_nats::connect(&nats_url)
            .await
            .context("failed to connect washlet NATS client")?,
    );
    let api_client = async_nats::connect(&nats_url)
        .await
        .context("failed to connect API NATS client")?;

    let cluster_host = ClusterHostBuilder::default()
        .with_host_group(HOST_GROUP)
        .with_environment(ENVIRONMENT)
        .with_nats_client(washlet_client)
        .build()
        .context("failed to build cluster host")?;
    let host_id = cluster_host.host().id().to_string();

    // Subscribe (and flush, so the server has registered the SUB) before the
    // host starts publishing.
    let heartbeat_sub = api_client
        .subscribe(heartbeat_subject(&host_id))
        .await
        .context("failed to subscribe to heartbeats")?;
    api_client
        .flush()
        .await
        .context("failed to flush heartbeat subscription")?;

    let (_host, shutdown) = cluster_host
        .start()
        .await
        .context("failed to start cluster host")?;

    let harness = TestHarness {
        api_client,
        host_id,
        heartbeat_sub,
        shutdown: Box::pin(shutdown),
        _container: container,
    };
    harness.wait_for_api().await?;
    Ok(harness)
}

impl TestHarness {
    fn subject(&self, command: &str) -> String {
        rpc_subject(&self.host_id, command)
    }

    /// The washlet registers its API subscription inside a spawned task, so a
    /// request sent immediately after `start()` can race it and get "no
    /// responders". Probe with a harmless status query until the API answers.
    async fn wait_for_api(&self) -> Result<()> {
        let probe = v2::WorkloadStatusRequest {
            workload_id: "washlet-api-e2e-probe".to_string(),
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let res: Result<v2::WorkloadStatusResponse> =
                    rpc(&self.api_client, self.subject("workload.status"), &probe).await;
                if res.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .context("washlet API did not come up within 10s")
    }

    async fn start(&self, request: &v2::WorkloadStartRequest) -> Result<v2::WorkloadStatus> {
        let resp: v2::WorkloadStartResponse =
            rpc(&self.api_client, self.subject("workload.start"), request).await?;
        status_of(resp.workload_status)
    }

    async fn status(&self, workload_id: &str) -> Result<v2::WorkloadStatus> {
        let resp: v2::WorkloadStatusResponse = rpc(
            &self.api_client,
            self.subject("workload.status"),
            &v2::WorkloadStatusRequest {
                workload_id: workload_id.to_string(),
            },
        )
        .await?;
        status_of(resp.workload_status)
    }

    async fn stop(&self, workload_id: &str) -> Result<v2::WorkloadStatus> {
        let resp: v2::WorkloadStopResponse = rpc(
            &self.api_client,
            self.subject("workload.stop"),
            &v2::WorkloadStopRequest {
                workload_id: workload_id.to_string(),
            },
        )
        .await?;
        status_of(resp.workload_status)
    }

    async fn heartbeat(&self) -> Result<v2::HostHeartbeat> {
        let reply = self
            .api_client
            .request(self.subject("heartbeat"), Vec::new().into())
            .await
            .context("heartbeat request failed")?;
        serde_json::from_slice(&reply.payload).context("failed to deserialize heartbeat")
    }

    async fn shutdown(self) -> Result<()> {
        self.shutdown.await.context("washlet shutdown failed")
    }
}

/// One washlet API round trip: JSON-encoded request out, JSON-decoded
/// response back, matching `to_api`/`from_api` on the host side.
async fn rpc<Req, Resp>(client: &async_nats::Client, subject: String, req: &Req) -> Result<Resp>
where
    Req: Serialize,
    Resp: DeserializeOwned,
{
    let payload = serde_json::to_vec(req).context("failed to serialize request")?;
    let reply = client
        .request(subject, payload.into())
        .await
        .context("washlet API request failed")?;
    serde_json::from_slice(&reply.payload).context("failed to deserialize response")
}

fn status_of(status: Option<v2::WorkloadStatus>) -> Result<v2::WorkloadStatus> {
    status.context("response missing workload_status")
}

/// A minimal workload start request: no components, no service, no OCI
/// pulls — for tests about workload-ID bookkeeping, not resolution.
fn empty_start_request(workload_id: &str) -> v2::WorkloadStartRequest {
    v2::WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Some(v2::Workload {
            namespace: "default".to_string(),
            name: "washlet-api-e2e".to_string(),
            annotations: Default::default(),
            service: None,
            wit_world: None,
            volumes: vec![],
        }),
    }
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn duplicate_workload_id_rejected_over_washlet_api() -> Result<()> {
    let harness = setup().await?;
    let workload_id = "washlet-api-e2e-duplicate";

    // First start claims the ID.
    let first = harness.start(&empty_start_request(workload_id)).await?;
    assert_eq!(
        first.workload_state(),
        v2::WorkloadState::Running,
        "first start should run: {}",
        first.message
    );

    // Re-sending the same workload ID must be rejected, not silently
    // replace the running workload.
    let duplicate = harness.start(&empty_start_request(workload_id)).await?;
    assert_eq!(duplicate.workload_state(), v2::WorkloadState::Error);
    assert_eq!(duplicate.message, "Workload ID already exists");
    // The rejection must echo the ID: `reconcilePlacement` records it from
    // the response without checking the state, then converges via status.
    assert_eq!(duplicate.workload_id, workload_id);

    // The rejected start left the original workload untouched.
    assert_eq!(
        harness.status(workload_id).await?.workload_state(),
        v2::WorkloadState::Running
    );

    // An explicit stop releases the ID...
    assert_eq!(
        harness.stop(workload_id).await?.workload_state(),
        v2::WorkloadState::Stopping
    );

    // ...after which the same ID starts cleanly (the explicit replace flow).
    let restarted = harness.start(&empty_start_request(workload_id)).await?;
    assert_eq!(
        restarted.workload_state(),
        v2::WorkloadState::Running,
        "restart after stop should run: {}",
        restarted.message
    );

    harness.shutdown().await
}

/// The operator's `finalize` stops a workload whose ID may already be gone
/// (host restarted, or a previous stop raced the finalizer retry). Teardown
/// must be idempotent: unknown IDs answer NOT_FOUND, they don't error and
/// they don't create state.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn unknown_workload_ids_answer_not_found() -> Result<()> {
    let harness = setup().await?;
    let workload_id = "washlet-api-e2e-unknown";

    let status = harness.status(workload_id).await?;
    assert_eq!(status.workload_state(), v2::WorkloadState::NotFound);
    assert_eq!(status.workload_id, workload_id);

    let stop = harness.stop(workload_id).await?;
    assert_eq!(stop.workload_state(), v2::WorkloadState::NotFound);
    assert_eq!(stop.workload_id, workload_id);

    // The probes above must not have materialized an entry.
    assert_eq!(
        harness.status(workload_id).await?.workload_state(),
        v2::WorkloadState::NotFound
    );

    harness.shutdown().await
}

/// An OCI pull failure happens in the washlet, before the host reserves the
/// workload ID. The Error response must not consume the ID: a corrected
/// start with the same ID succeeds without an intervening stop.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn pull_failure_does_not_consume_workload_id() -> Result<()> {
    let harness = setup().await?;
    let workload_id = "washlet-api-e2e-pull-failure";

    // A closed local port makes the pull fail fast with connection refused.
    let mut request = empty_start_request(workload_id);
    request
        .workload
        .as_mut()
        .expect("request has a workload")
        .wit_world = Some(v2::WitWorld {
        components: vec![v2::Component {
            name: "unpullable".to_string(),
            image: "127.0.0.1:1/nope:latest".to_string(),
            ..Default::default()
        }],
        host_interfaces: vec![],
    });

    let failed = harness.start(&request).await?;
    assert_eq!(failed.workload_state(), v2::WorkloadState::Error);
    assert!(
        failed.message.contains("failed to pull component image"),
        "unexpected failure message: {}",
        failed.message
    );

    // The failed pull never reached the host, so the ID is still free.
    assert_eq!(
        harness.status(workload_id).await?.workload_state(),
        v2::WorkloadState::NotFound
    );
    let started = harness.start(&empty_start_request(workload_id)).await?;
    assert_eq!(
        started.workload_state(),
        v2::WorkloadState::Running,
        "start after pull failure should run: {}",
        started.message
    );

    harness.shutdown().await
}

/// Host registration contract: the published heartbeat (which the operator's
/// host controller consumes to create Host CRDs) and the `heartbeat` RPC
/// (which reconciles refresh from) carry the host identity, the `hostgroup`
/// label placement matches on, and the environment recorded verbatim for
/// tenant attribution. The workload count tracks running workloads.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn heartbeat_reports_identity_and_workload_count() -> Result<()> {
    let mut harness = setup().await?;

    let published = tokio::time::timeout(Duration::from_secs(10), harness.heartbeat_sub.next())
        .await
        .context("no heartbeat published within 10s")?
        .context("heartbeat subscription closed")?;
    let published: v2::HostHeartbeat = serde_json::from_slice(&published.payload)
        .context("failed to deserialize published heartbeat")?;
    assert_eq!(published.id, harness.host_id);
    assert_eq!(
        published.labels.get("hostgroup").map(String::as_str),
        Some(HOST_GROUP)
    );
    assert_eq!(published.environment, ENVIRONMENT);
    assert_eq!(published.workload_count, 0);

    let workload_id = "washlet-api-e2e-heartbeat";
    let started = harness.start(&empty_start_request(workload_id)).await?;
    assert_eq!(
        started.workload_state(),
        v2::WorkloadState::Running,
        "start should run: {}",
        started.message
    );

    let refreshed = harness.heartbeat().await?;
    assert_eq!(refreshed.id, harness.host_id);
    assert_eq!(refreshed.workload_count, 1);

    harness.shutdown().await
}
