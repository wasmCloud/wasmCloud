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
//! - A start claims its workload ID before fetching anything, so a stop or a
//!   status arriving mid-start reports the workload rather than a gap; a start
//!   that then fails (OCI pull failure in the washlet) gives the ID back.
//! - `finalize`'s idempotent teardown: stop/status of an unknown ID answer
//!   `WORKLOAD_STATE_NOT_FOUND` instead of erroring.
//! - Shutdown's drain: the commands already running get `COMMAND_DRAIN_TIMEOUT`
//!   to finish and are abandoned after it, so one stalled on a pull cannot hold
//!   a terminating host past the grace period its pod was given.
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
use wash_runtime::washlet::{
    COMMAND_DRAIN_TIMEOUT, ClusterHostBuilder, heartbeat_subject, rpc_subject, types::v2,
};

const HOST_GROUP: &str = "e2e";
const ENVIRONMENT: &str = "e2e-env";
/// Slack over the drain for the abort to unwind and `host.stop()` to run, on a
/// runner busy with the rest of the suite.
const ABANDON_MARGIN: Duration = Duration::from_secs(10);

struct TestHarness {
    api_client: async_nats::Client,
    host_id: String,
    /// Subscribed before the host starts, so it observes the heartbeat the
    /// washlet publishes on its immediate first tick.
    heartbeat_sub: async_nats::Subscriber,
    shutdown: Pin<Box<dyn Future<Output = Result<()>> + Send>>,
    _container: ContainerAsync<GenericImage>,
}

/// A NATS container and a washlet on it, configured the way the code under
/// test is: `with_*` setters over the knobs a test cares about, defaults for
/// the rest.
#[derive(Default)]
struct TestHarnessBuilder {
    heartbeat_interval: Option<Duration>,
    max_concurrent_starts: Option<usize>,
}

impl TestHarnessBuilder {
    fn with_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.heartbeat_interval = Some(interval);
        self
    }

    fn with_max_concurrent_starts(mut self, starts: usize) -> Self {
        self.max_concurrent_starts = Some(starts);
        self
    }

    async fn start(self) -> Result<TestHarness> {
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

        let mut builder = ClusterHostBuilder::default()
            .with_host_group(HOST_GROUP)
            .with_environment(ENVIRONMENT)
            .with_nats_client(washlet_client);
        if let Some(interval) = self.heartbeat_interval {
            builder = builder.with_heartbeat_interval(interval);
        }
        if let Some(starts) = self.max_concurrent_starts {
            builder = builder.with_max_concurrent_starts(starts);
        }
        let cluster_host = builder.build().context("failed to build cluster host")?;
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
}

/// The harness every test that needs no particular configuration starts from.
async fn setup() -> Result<TestHarness> {
    TestHarness::builder().start().await
}

impl TestHarness {
    fn builder() -> TestHarnessBuilder {
        TestHarnessBuilder::default()
    }

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

/// A registry that completes the TCP connect and then says nothing, so a pull
/// against it hangs where a refused connection would fail fast.
struct StallingRegistry {
    addr: std::net::SocketAddr,
    reached: Arc<std::sync::atomic::AtomicUsize>,
    accept: tokio::task::JoinHandle<()>,
}

impl StallingRegistry {
    async fn bind() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind stalling registry")?;
        let addr = listener
            .local_addr()
            .context("failed to read stalling registry address")?;
        let reached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accept = tokio::spawn({
            let reached = Arc::clone(&reached);
            async move {
                // Accepted and held, never answered.
                let mut accepted = Vec::new();
                while let Ok((stream, _)) = listener.accept().await {
                    reached.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    accepted.push(stream);
                }
            }
        });
        Ok(Self {
            addr,
            reached,
            accept,
        })
    }

    /// Connections accepted so far: one per start that got as far as pulling.
    fn reached(&self) -> usize {
        self.reached.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Fires a start whose only component pulls from here, so it hangs. The
    /// handle is the caller's to abort; the reply never comes.
    fn spawn_start(&self, harness: &TestHarness, workload_id: &str) -> tokio::task::JoinHandle<()> {
        let mut request = empty_start_request(workload_id);
        request.workload = request.workload.map(|workload| v2::Workload {
            wit_world: Some(v2::WitWorld {
                components: vec![v2::Component {
                    name: "stalled".to_string(),
                    image: format!("{}/{workload_id}:latest", self.addr),
                    ..Default::default()
                }],
                host_interfaces: vec![],
            }),
            ..workload
        });
        let client = harness.api_client.clone();
        let subject = harness.subject("workload.start");
        tokio::spawn(async move {
            let _: Result<v2::WorkloadStartResponse> = rpc(&client, subject, &request).await;
        })
    }
}

impl Drop for StallingRegistry {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// Polls `condition` until it holds or `limit` elapses.
async fn wait_for(limit: Duration, mut condition: impl FnMut() -> bool) -> Result<()> {
    tokio::time::timeout(limit, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("condition did not hold in time")
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
    assert!(
        duplicate
            .message
            .contains(&format!("Workload ID [{workload_id}] already exists")),
        "unexpected rejection message: {}",
        duplicate.message
    );
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

/// An OCI pull failure leaves the workload ID free. The ID is claimed before
/// the pull, so this pins the release on the failure path: a corrected start
/// with the same ID succeeds without an intervening stop.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn pull_failure_does_not_consume_workload_id() -> Result<()> {
    let harness = setup().await?;
    let workload_id = "washlet-api-e2e-pull-failure";

    // A closed local port makes the pull fail fast with connection refused.
    let mut request = empty_start_request(workload_id);
    request.workload = request.workload.map(|workload| v2::Workload {
        wit_world: Some(v2::WitWorld {
            components: vec![v2::Component {
                name: "unpullable".to_string(),
                image: "127.0.0.1:1/nope:latest".to_string(),
                ..Default::default()
            }],
            host_interfaces: vec![],
        }),
        ..workload
    });

    let failed = harness.start(&request).await?;
    assert_eq!(failed.workload_state(), v2::WorkloadState::Error);
    // The operator surfaces this message verbatim, so it has to say which
    // component wanted which image, not just that some pull failed.
    for expected in [
        "failed to pull image for component 'unpullable'",
        "127.0.0.1:1/nope:latest",
    ] {
        assert!(
            failed.message.contains(expected),
            "failure message should contain {expected:?}, got: {}",
            failed.message
        );
    }

    // The failed start gave the ID back, so it is free again.
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

/// A start holds the washlet's request handler for as long as its image pull
/// and compilation take. Heartbeats have to keep flowing while it does: the
/// operator deletes a host it has not heard from inside its unreachable
/// window, and every workload on that host goes with it.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn heartbeats_continue_while_a_start_is_in_flight() -> Result<()> {
    const HEARTBEAT: Duration = Duration::from_millis(200);
    let mut harness = TestHarness::builder()
        .with_heartbeat_interval(HEARTBEAT)
        .start()
        .await?;
    let registry = StallingRegistry::bind().await?;

    let start = registry.spawn_start(&harness, "washlet-api-e2e-slow-start");

    // Wait until the start is genuinely stuck on the pull, so the window below
    // measures a busy host rather than an idle one.
    wait_for(Duration::from_secs(10), || registry.reached() > 0)
        .await
        .context("no start reached the stalling registry")?;

    // Drop whatever was published before the start: those heartbeats sit in the
    // subscriber's buffer and would count toward the window on their own. The
    // budget has to be long enough for the reader task to hand over what it has
    // already taken off the socket, and `Ok(None)` means the subscription
    // closed, which no amount of waiting will improve.
    while let Ok(Some(_)) =
        tokio::time::timeout(Duration::from_millis(50), harness.heartbeat_sub.next()).await
    {}

    // Count what the host publishes while that start is stuck on the pull.
    let window = HEARTBEAT * 6;
    let deadline = tokio::time::Instant::now() + window;
    let mut heard = 0usize;
    while let Ok(Some(_)) = tokio::time::timeout_at(deadline, harness.heartbeat_sub.next()).await {
        heard += 1;
    }

    assert!(
        heard >= 3,
        "only {heard} heartbeats in {window:?} while a start was in flight; \
         the request handler is holding the washlet's select loop"
    );

    start.abort();
    harness.shutdown().await
}

/// Handling requests off the loop let starts overlap, so something has to cap
/// how many images a host pulls and compiles at once. Only starts wait on it:
/// a stop or a status stuck behind a slow start is a host that looks wedged.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn concurrent_workload_starts_are_bounded() -> Result<()> {
    const LIMIT: usize = 2;
    let harness = TestHarness::builder()
        .with_max_concurrent_starts(LIMIT)
        .start()
        .await?;

    // Each start that gets a permit reaches the registry and hangs there, so
    // the connections it accepts count the starts in flight.
    let registry = StallingRegistry::bind().await?;

    let inflight: Vec<_> = (0..LIMIT + 2)
        .map(|i| registry.spawn_start(&harness, &format!("washlet-api-e2e-bounded-{i}")))
        .collect();

    // Wait for the permitted starts to arrive rather than guessing at a delay,
    // then hold to see whether any more follow them through.
    wait_for(Duration::from_secs(10), || registry.reached() >= LIMIT)
        .await
        .context("the permitted starts never reached the registry")?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let reached_now = registry.reached();
    assert!(
        reached_now <= LIMIT,
        "{reached_now} starts reached the registry at once, past the cap of {LIMIT}"
    );

    // A status query must not be stuck behind the starts holding the permits.
    tokio::time::timeout(
        Duration::from_secs(5),
        harness.status("washlet-api-e2e-bounded-unknown"),
    )
    .await
    .context("status queued behind the in-flight starts")??;

    for task in inflight {
        task.abort();
    }
    harness.shutdown().await
}

/// A start waiting for a concurrency permit has claimed its id too. The wait is
/// time like any other in which a stop can arrive, and a stop that finds no
/// workload tells the operator the teardown is done — so the record goes while
/// the start is still queued to run it.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn a_stop_during_a_queued_start_reports_the_workload() -> Result<()> {
    // One permit, held by a start that never finishes pulling, so the workload
    // below is still waiting for it.
    let harness = TestHarness::builder()
        .with_max_concurrent_starts(1)
        .start()
        .await?;
    let registry = StallingRegistry::bind().await?;

    let holder = registry.spawn_start(&harness, "washlet-api-e2e-permit-holder");
    wait_for(Duration::from_secs(10), || registry.reached() > 0)
        .await
        .context("the first start never reached the stalling registry")?;

    let queued_id = "washlet-api-e2e-permit-queued";
    let queued = registry.spawn_start(&harness, queued_id);

    // Wait for the queued start to claim its id. It cannot be pulling — the one
    // permit is taken — so this is the claim and nothing else, and until it
    // lands there is no race for the stop below to lose.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match harness.status(queued_id).await {
                Ok(observed) if observed.workload_state() != v2::WorkloadState::NotFound => return,
                _ => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await
    .context("a start queued for a permit never claimed its workload id")?;

    let stopped = tokio::time::timeout(Duration::from_secs(5), harness.stop(queued_id))
        .await
        .context("stop did not answer while a start was queued")??;
    assert_ne!(
        stopped.workload_state(),
        v2::WorkloadState::NotFound,
        "stop reported a queued start as already gone: {}",
        stopped.message
    );

    holder.abort();
    queued.abort();
    harness.shutdown().await
}

/// A workload id is claimed before its images are fetched, so a stop arriving
/// while a start is still pulling finds the workload rather than a gap. It has
/// to: NOT_FOUND tells the operator the teardown is complete, so it drops the
/// record and the start goes on to run a workload nothing is tracking. The stop
/// answers promptly — the start owns the teardown and finishes it — so the
/// operator is neither misled nor left waiting out its own timeout.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn a_stop_during_a_start_reports_the_workload() -> Result<()> {
    let harness = setup().await?;
    let workload_id = "washlet-api-e2e-stop-races-start";

    let registry = StallingRegistry::bind().await?;

    let start = registry.spawn_start(&harness, workload_id);

    wait_for(Duration::from_secs(10), || registry.reached() > 0)
        .await
        .context("the start never reached the stalling registry")?;

    let stopped = tokio::time::timeout(Duration::from_secs(5), harness.stop(workload_id))
        .await
        .context("stop did not answer while a start was in flight")??;
    assert_ne!(
        stopped.workload_state(),
        v2::WorkloadState::NotFound,
        "stop reported a workload that is on its way up as already gone: {}",
        stopped.message
    );

    // And a status in the same window reports it too, rather than answering
    // for an id the host does not know yet.
    let observed = tokio::time::timeout(Duration::from_secs(5), harness.status(workload_id))
        .await
        .context("status did not answer while a start was in flight")??;
    assert_ne!(
        observed.workload_state(),
        v2::WorkloadState::NotFound,
        "status reported a workload that is on its way up as missing: {}",
        observed.message
    );

    start.abort();
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

/// Shutdown waits for the commands already running — but a start stalled on an
/// unreachable registry runs to its own pull timeout, which is minutes. Waiting
/// that out means the pod is killed before `host.stop()` unbinds anything, so
/// the drain is bounded and whatever outlasts it is abandoned.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn shutdown_abandons_a_start_that_outlasts_the_drain() -> Result<()> {
    let mut harness = setup().await?;
    let registry = StallingRegistry::bind().await?;

    let start = registry.spawn_start(&harness, "washlet-api-e2e-stalled-shutdown");
    wait_for(Duration::from_secs(10), || registry.reached() > 0)
        .await
        .context("no start reached the stalling registry")?;

    // Awaited here rather than through `TestHarness::shutdown` so the elapsed
    // time covers the drain alone, not the container teardown behind it.
    let began = std::time::Instant::now();
    let stopped = (&mut harness.shutdown).await;
    let drained_in = began.elapsed();
    start.abort();
    stopped.context("shutdown failed with a start still in flight")?;

    assert!(
        drained_in >= COMMAND_DRAIN_TIMEOUT,
        "shutdown gave the start in flight {drained_in:?}, short of the {COMMAND_DRAIN_TIMEOUT:?} drain"
    );
    assert!(
        drained_in < COMMAND_DRAIN_TIMEOUT + ABANDON_MARGIN,
        "shutdown held for {drained_in:?} on a start stalled at its registry; \
         it must abandon the start once the {COMMAND_DRAIN_TIMEOUT:?} drain is up"
    );
    Ok(())
}
