use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::component_source::{ComponentSource, LoadedComponent};
use crate::host::{Host, HostApi, HostConfig, WorkloadReservation};
use crate::oci::{self, OciConfig};
use crate::plugin::HostPlugin;
use anyhow::{Context as _, anyhow};
use async_trait::async_trait;
use futures::{FutureExt as _, StreamExt as _};
use tokio::sync::oneshot;
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{debug, error, info, instrument, warn};

pub const HOST_API_PREFIX: &str = "runtime.host";
pub const OPERATOR_API_PREFIX: &str = "runtime.operator";

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

const CLEANUP_INTERVAL: Duration = Duration::from_secs(300);
const CLEANUP_AGE: Duration = Duration::from_secs(3600);

const CONTROL_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(1);

/// Maximum accepted start commands, including starts waiting for a native permit.
pub const MAX_PENDING_STARTS: usize = 64;
const MAX_PENDING_QUERIES: usize = 64;
const MAX_PENDING_REPLIES: usize = MAX_PENDING_STARTS + MAX_PENDING_QUERIES;

/// Covers every bounded shutdown step, including cleanup grace and scheduling slack.
fn attachment_shutdown_timeout() -> Duration {
    CONTROL_IO_TIMEOUT * 2
        + COMMAND_DRAIN_TIMEOUT
        + COMMAND_ABORT_TIMEOUT
        + crate::timeouts::plugin_stop()
        + Duration::from_secs(2)
}

/// Built-in workload operations on the same host as the control loop.
/// A handler can add policy or logging and then delegate to these methods.
#[derive(Clone)]
pub struct HostControlDefaults {
    host: Arc<Host>,
    starts: Arc<tokio::sync::Semaphore>,
    _control: Option<Arc<crate::host::HostControlLease>>,
}

impl HostControlDefaults {
    /// Perform the runtime's normal reservation, OCI pull and workload start.
    pub async fn start(
        &self,
        request: types::v2::WorkloadStartRequest,
    ) -> anyhow::Result<types::v2::WorkloadStartResponse> {
        workload_start(
            self.host.as_ref(),
            request,
            self.host.config(),
            &self.starts,
        )
        .await
    }

    /// Perform the runtime's normal stop.
    pub async fn stop(
        &self,
        request: types::v2::WorkloadStopRequest,
    ) -> anyhow::Result<types::v2::WorkloadStopResponse> {
        workload_stop(self.host.as_ref(), request).await
    }

    /// Perform the runtime's normal status lookup.
    pub async fn status(
        &self,
        request: types::v2::WorkloadStatusRequest,
    ) -> anyhow::Result<types::v2::WorkloadStatusResponse> {
        workload_status(self.host.as_ref(), request).await
    }
}

/// Optional command customization. The default implementation of each method
/// delegates to the runtime; overrides can do work before or after delegation.
///
/// Each override is responsible for its product's admission and workload
/// ownership. Errors become typed workload error replies. Delegating to
/// [`HostControlDefaults::start`] reserves the workload id and then waits for
/// the host's start permit; an override that does work before delegating
/// bounds that work itself. The loop admits at most [`MAX_PENDING_STARTS`]
/// start tasks, including overrides. Stop and status never wait for a start permit.
///
/// A start or status the loop has no room for is not answered: its caller
/// times out and retries, as it does when the host is unreachable. An error
/// reply would name a workload this host never claimed, and a status would
/// report a running workload as failed. A stop is never shed.
///
/// Methods run concurrently and may be cancelled after the shutdown drain.
/// Keep side effects cancellation-safe; do not detach work from these futures.
#[async_trait]
pub trait HostCommandHandler: Send + Sync {
    async fn start(
        &self,
        defaults: &HostControlDefaults,
        request: types::v2::WorkloadStartRequest,
    ) -> anyhow::Result<types::v2::WorkloadStartResponse> {
        defaults.start(request).await
    }
    async fn stop(
        &self,
        defaults: &HostControlDefaults,
        request: types::v2::WorkloadStopRequest,
    ) -> anyhow::Result<types::v2::WorkloadStopResponse> {
        defaults.stop(request).await
    }
    async fn status(
        &self,
        defaults: &HostControlDefaults,
        request: types::v2::WorkloadStatusRequest,
    ) -> anyhow::Result<types::v2::WorkloadStatusResponse> {
        defaults.status(request).await
    }
}

type ControlCompletion =
    futures::future::Shared<futures::future::BoxFuture<'static, Result<(), Arc<anyhow::Error>>>>;

/// The runtime's NATS control loop attached to a caller-owned, already-started host.
/// Call [`Self::shutdown`] before stopping that host. Dropping this handle aborts
/// the loop and its tasks; graceful shutdown requires awaiting `shutdown`.
pub struct AttachedHostControl {
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    abort: AbortHandle,
    completion: ControlCompletion,
}

/// Configure control for an existing host without changing its engine,
/// plugins, ingress, or lifecycle ownership.
pub struct AttachedHostControlBuilder {
    host: Arc<Host>,
    nats_client: Arc<async_nats::Client>,
    options: HostControlLoopOptions,
}

impl AttachedHostControlBuilder {
    /// Override the hostgroup in control heartbeats only. Other labels and
    /// the environment still come from the caller's host.
    pub fn with_host_group(mut self, host_group: impl Into<String>) -> Self {
        self.options.host_group = Some(Arc::from(host_group.into()));
        self
    }

    /// Inject product admission and ownership for remote commands.
    pub fn with_handler(mut self, handler: Arc<dyn HostCommandHandler>) -> Self {
        self.options.handler = Some(handler);
        self
    }

    /// Set the heartbeat cadence. Zero retains the default interval.
    pub fn with_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.options.heartbeat_interval = if interval.is_zero() {
            HEARTBEAT_INTERVAL
        } else {
            interval
        };
        self
    }

    /// Bound native starts, including handlers delegating to
    /// [`HostControlDefaults::start`]. Work before delegation is bounded by
    /// the handler. Zero is read as one, as it is for
    /// [`ClusterHostBuilder::with_max_concurrent_starts`].
    pub fn with_max_concurrent_starts(mut self, starts: usize) -> Self {
        self.options.max_concurrent_starts = starts.max(1);
        self
    }

    /// Set cleanup for the host's configured OCI cache. Zero retains the
    /// default interval; a host without a cache never starts a cleanup timer.
    pub fn with_artifact_cleaner(mut self, frequency: Duration, max_age: Duration) -> Self {
        self.options.cleanup_interval = if frequency.is_zero() {
            CLEANUP_INTERVAL
        } else {
            frequency
        };
        self.options.cleanup_age = max_age;
        self
    }

    /// Beat this probe whenever the control loop turns.
    pub fn with_liveness(mut self, liveness: Arc<crate::host::probes::Liveness>) -> Self {
        self.options.liveness = Some(liveness);
        self
    }

    /// Verify the API subscription with a broker round trip before returning.
    /// The client must publish to `runtime.host.{id}.__control.ready` and
    /// subscribe to `runtime.host.{id}.>`, and must receive its own messages:
    /// one connected with `no_echo` never sees the marker and cannot attach.
    /// A broker that refuses either permission is reported as a timeout.
    /// Failure leaves the host running.
    pub async fn attach(self) -> anyhow::Result<AttachedHostControl> {
        self.options.validate()?;
        let control = self.host.acquire_control()?;
        let (subscription, pending) =
            subscribe_host(&self.host, &self.nats_client, self.options.startup_timeout).await?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = spawn_control_loop(
            self.host,
            self.nats_client,
            subscription,
            pending,
            control,
            shutdown_rx,
            self.options,
        );
        Ok(AttachedHostControl::new(shutdown_tx, task))
    }
}

impl AttachedHostControl {
    /// Configure a native control loop on the same running host. Defaults
    /// match `ClusterHost`; heartbeat labels come from the host unless overridden.
    pub fn builder(
        host: Arc<Host>,
        nats_client: Arc<async_nats::Client>,
    ) -> AttachedHostControlBuilder {
        AttachedHostControlBuilder {
            host,
            nats_client,
            options: HostControlLoopOptions::default(),
        }
    }

    /// Verify registration before returning, so the first request can be served.
    /// See [`AttachedHostControlBuilder::attach`] for the client's permissions.
    pub async fn attach(
        host: Arc<Host>,
        nats_client: Arc<async_nats::Client>,
        host_group: impl Into<String>,
        handler: Option<Arc<dyn HostCommandHandler>>,
    ) -> anyhow::Result<Self> {
        let mut builder = Self::builder(host, nats_client).with_host_group(host_group);
        if let Some(handler) = handler {
            builder = builder.with_handler(handler);
        }
        builder.attach().await
    }

    fn new(shutdown_tx: oneshot::Sender<()>, task: JoinHandle<anyhow::Result<()>>) -> Self {
        let abort = task.abort_handle();
        let completion = async move {
            task.await
                .context("host control task failed")
                .and_then(std::convert::identity)
                .map_err(Arc::new)
        }
        .boxed()
        .shared();
        Self {
            shutdown: Mutex::new(Some(shutdown_tx)),
            abort,
            completion,
        }
    }

    /// Observe an unexpected exit without requesting shutdown. Multiple
    /// observers receive the same result, and cancelling a wait is safe.
    pub async fn stopped(&self) -> anyhow::Result<()> {
        self.completion
            .clone()
            .await
            .map_err(|error| anyhow!("{error:#}"))
    }

    /// Stop subscriptions and drain or abort commands within the runtime's bounds.
    /// The caller's host remains running.
    ///
    /// Cancellation of this wait does not detach the loop; another call can
    /// finish waiting. Concurrent callers all await the same completion.
    /// A forced abort is an error. Native starts retain their IDs until resource
    /// cleanup finishes; failed cleanup can be retried with a workload stop.
    /// Custom handlers remain responsible for recovery of their own side effects.
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        if let Some(tx) = self
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = tx.send(());
        }
        let timeout = attachment_shutdown_timeout();
        match tokio::time::timeout(timeout, self.stopped()).await {
            Ok(result) => result,
            Err(_) => {
                self.abort.abort();
                let _ = tokio::time::timeout(COMMAND_ABORT_TIMEOUT, self.stopped()).await;
                anyhow::bail!("attached host control did not stop within {timeout:?}");
            }
        }
    }
}

impl Drop for AttachedHostControl {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// How long the command loop may go quiet before it counts as stopped rather
/// than slow.
///
/// Three of whatever interval the host actually heartbeats on: the loop turns
/// at least once per interval, and a restart costs every workload on the host,
/// so this must not fire on one that is merely behind.
pub fn liveness_silence(heartbeat_interval: Duration) -> Duration {
    heartbeat_interval * 3
}
/// Most `workload.start` requests a host pulls and compiles at once, however
/// many cores it has. Past this the images held in memory cost more than the
/// extra parallelism buys.
const MAX_CONCURRENT_STARTS: usize = 4;
/// How long shutdown waits for in-flight commands before abandoning them.
///
/// A start stalled on an unreachable registry runs to its own pull timeout, and
/// waiting all of them out would hold shutdown for minutes — past the grace
/// period a terminating pod gets, so it is killed before `host.stop()` unbinds
/// anything. Better to abandon them and stop the host, which unbinds whatever
/// they had bound anyway.
/// Attached loops report abandoned commands as an error so the caller can
/// recover custom handler side effects. Cancelled native starts release their
/// reservations after resource cleanup, without stopping unrelated workloads.
pub const COMMAND_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long an aborted command is given to unwind before the host stops
/// regardless.
///
/// Separate from, and shorter than, [`COMMAND_DRAIN_TIMEOUT`]: that one waits
/// for commands to *finish*, this one only for already-aborted ones to reach an
/// await and let go. Together they bound the whole shutdown, which has to fit
/// inside the pod's termination grace period with room left for
/// `Host::stop` to unbind the plugins.
pub const COMMAND_ABORT_TIMEOUT: Duration = Duration::from_secs(2);

/// How many starts a host runs at once when nothing sets it.
///
/// Each permitted start ends in a Cranelift compile, and one compile spreads
/// itself over rayon's pool, so a permit is a floor on the cores it takes for
/// as long as it runs, not a ceiling — the pool's own size is what bounds the
/// rest. `available_parallelism` reads the cgroup quota a container is limited to,
/// and one core is left out of the count: a host that cannot poll its HTTP
/// accept loop or drain its NATS socket while it compiles is one Kubernetes
/// restarts out from under its workloads.
fn default_max_concurrent_starts() -> usize {
    std::thread::available_parallelism().map_or(1, |cores| starts_for_cores(cores.get()))
}

/// [`default_max_concurrent_starts`] against a known core count.
fn starts_for_cores(cores: usize) -> usize {
    cores.saturating_sub(1).clamp(1, MAX_CONCURRENT_STARTS)
}

pub mod types {
    pub mod v2 {
        // Committed output of `cargo xtask generate-protos` — regenerate
        // after changing /proto/wasmcloud/runtime/v2.
        // Generated by [`tonic-prost-build`]
        include!("generated/wasmcloud.runtime.v2.rs");
        // Generated by [`pbjson-build`]
        include!("generated/wasmcloud.runtime.v2.serde.rs");
    }
}

#[derive(Default)]
pub struct ClusterHostBuilder {
    host_builder: crate::host::HostBuilder,
    nats_client: Option<Arc<async_nats::Client>>,
    host_group: Option<String>,
    host_name: Option<String>,
    environment: Option<String>,
    heartbeat_interval: Option<Duration>,
    cleanup_interval: Option<Duration>,
    cleanup_age: Option<Duration>,
    host_config: Option<HostConfig>,
    max_concurrent_starts: Option<usize>,
    liveness: Option<Arc<crate::host::probes::Liveness>>,
}

impl ClusterHostBuilder {
    /// The interval this host will actually heartbeat on, resolved.
    ///
    /// Public so a caller sizing a liveness bound reads the number the host was
    /// built with rather than restating a default that can move underneath it.
    pub fn heartbeat_interval(&self) -> Duration {
        self.heartbeat_interval.unwrap_or(HEARTBEAT_INTERVAL)
    }

    /// Beat `liveness` every time the command loop turns, so `/livez` answers
    /// from whether this host is still servicing its control plane rather than
    /// from whether a socket accepted.
    pub fn with_liveness(mut self, liveness: Arc<crate::host::probes::Liveness>) -> Self {
        self.liveness = Some(liveness);
        self
    }

    pub fn with_host_group(mut self, host_group: impl AsRef<str>) -> Self {
        self.host_group = Some(host_group.as_ref().into());
        self
    }

    pub fn with_host_name(mut self, host_name: impl AsRef<str>) -> Self {
        self.host_name = Some(host_name.as_ref().into());
        self
    }

    /// Sets the environment the host advertises in its heartbeat. For
    /// in-cluster host pods this is the pod's namespace (sourced from
    /// the downward API); for external hosts it is whatever identifier
    /// the operator wants to attribute the host to.
    pub fn with_environment(mut self, environment: impl AsRef<str>) -> Self {
        self.environment = Some(environment.as_ref().into());
        self
    }

    pub fn with_host_config(mut self, host_config: HostConfig) -> Self {
        self.host_config = Some(host_config);
        self
    }

    pub fn with_host_builder(mut self, host_builder: crate::host::HostBuilder) -> Self {
        self.host_builder = host_builder;
        self
    }

    pub fn with_nats_client(mut self, nats_client: Arc<async_nats::Client>) -> Self {
        self.nats_client = Some(nats_client);
        self
    }

    pub fn with_plugin<T: HostPlugin>(mut self, plugin: Arc<T>) -> anyhow::Result<Self> {
        self.host_builder = self.host_builder.with_plugin(plugin)?;
        Ok(self)
    }

    /// Sets the operator's plugin binding declarations. See
    /// [`crate::host::HostBuilder::with_plugin_bindings`].
    pub fn with_plugin_bindings(mut self, bindings: crate::plugin::PluginBindings) -> Self {
        self.host_builder = self.host_builder.with_plugin_bindings(bindings);
        self
    }

    /// Every native (non-component) plugin registered so far. See
    /// [`crate::host::HostBuilder::native_plugins`].
    #[cfg(feature = "host-component-plugins")]
    pub fn native_plugins(&self) -> std::collections::HashMap<&'static str, Arc<dyn HostPlugin>> {
        self.host_builder.native_plugins()
    }

    /// The HTTP handler registered so far, if any. See
    /// [`crate::host::HostBuilder::http_handler`].
    #[cfg(feature = "host-component-plugins")]
    pub fn http_handler(&self) -> Option<Arc<dyn crate::host::http::HostHandler>> {
        self.host_builder.http_handler()
    }

    /// Reference for a component plugin built before the cluster host.
    #[cfg(feature = "host-component-plugins")]
    pub fn host_ref(&self) -> crate::host::HostRef {
        self.host_builder.host_ref()
    }

    /// Registers the multiplexed plugin set. See
    /// [`crate::host::HostBuilder::with_multiplexed_plugins`].
    #[cfg(feature = "wasm_component_model_implements")]
    pub fn with_multiplexed_plugins(mut self) -> anyhow::Result<Self> {
        self.host_builder = self.host_builder.with_multiplexed_plugins()?;
        Ok(self)
    }

    /// A zero frequency is read as unset and takes the default. Passing it on
    /// would panic `tokio::time::interval` inside the spawned task, where the
    /// host reports itself started and then serves nothing; clamping it to some
    /// tiny period instead trades that for the cleanup walking the OCI cache
    /// thousands of times a second, inline in the loop that serves requests.
    pub fn with_artifact_cleaner(mut self, frequency: Duration, max_age: Duration) -> Self {
        self.cleanup_interval = (!frequency.is_zero()).then_some(frequency);
        self.cleanup_age = Some(max_age);
        self
    }

    /// Sets how often the host publishes to `runtime.operator.heartbeat.{id}`.
    /// It has to stay well inside the operator's unreachable window, which
    /// deletes a host it has not heard from along with its workloads.
    /// Zero is read as unset and takes the default, for the same reason as
    /// [`ClusterHostBuilder::with_artifact_cleaner`].
    pub fn with_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.heartbeat_interval = (!interval.is_zero()).then_some(interval);
        self
    }

    /// Caps how many `workload.start` requests this host serves at once,
    /// overriding [`default_max_concurrent_starts`].
    /// Zero is raised to one: a host that cannot start anything is not a host.
    pub fn with_max_concurrent_starts(mut self, starts: usize) -> Self {
        self.max_concurrent_starts = Some(starts.max(1));
        self
    }

    pub fn with_engine(mut self, engine: crate::engine::Engine) -> Self {
        self.host_builder = self.host_builder.with_engine(engine);
        self
    }

    pub fn with_meters(mut self, meters: crate::observability::Meters) -> Self {
        self.host_builder = self.host_builder.with_meters(meters);
        self
    }

    pub fn with_http_handler(
        mut self,
        http_handler: Arc<dyn crate::host::http::HostHandler>,
    ) -> Self {
        self.host_builder = self.host_builder.with_http_handler(http_handler);
        self
    }

    pub fn build(self) -> anyhow::Result<ClusterHost> {
        let Some(nats_client) = self.nats_client else {
            anyhow::bail!("nats_client is required");
        };
        let Some(host_group) = self.host_group else {
            anyhow::bail!("host_group is required");
        };

        let mut builder = self
            .host_builder
            .with_label("hostgroup", host_group.clone());

        if let Some(host_name) = self.host_name {
            builder = builder.with_hostname(host_name)
        }

        if let Some(environment) = self.environment {
            builder = builder.with_environment(environment);
        }

        if let Some(host_config) = self.host_config {
            builder = builder.with_config(host_config);
        }

        let heartbeat_interval = self.heartbeat_interval.unwrap_or(HEARTBEAT_INTERVAL);
        let host = builder.build()?;
        Ok(ClusterHost {
            prepared_host: host,
            nats_client,
            heartbeat_interval,
            cleanup_interval: self.cleanup_interval.unwrap_or(CLEANUP_INTERVAL),
            cleanup_age: self.cleanup_age.unwrap_or(CLEANUP_AGE),
            max_concurrent_starts: self
                .max_concurrent_starts
                .unwrap_or_else(default_max_concurrent_starts),
            liveness: self.liveness,
        })
    }
}

/// Why the command loop stopped turning. All run the same shutdown; they
/// differ only in what the host reports afterwards.
enum Ended {
    /// The cleanup future was awaited.
    Requested,
    /// The HTTP ingress' accept loop returned.
    IngressStopped,
    /// A command task panicked or was cancelled.
    CommandPanicked(tokio::task::JoinError),
    /// The NATS API subscription ended and cannot serve further commands.
    SubscriptionClosed,
    /// The reply publisher returned while the loop still held its queue, so
    /// commands would run and never be answered.
    ReplyPublisherStopped,
}

pub struct ClusterHost {
    prepared_host: Host,
    nats_client: Arc<async_nats::Client>,
    heartbeat_interval: Duration,
    cleanup_interval: Duration,
    cleanup_age: Duration,
    max_concurrent_starts: usize,
    liveness: Option<Arc<crate::host::probes::Liveness>>,
}

struct HostControlLoopOptions {
    handler: Option<Arc<dyn HostCommandHandler>>,
    host_group: Option<Arc<str>>,
    heartbeat_interval: Duration,
    cleanup_interval: Duration,
    cleanup_age: Duration,
    max_concurrent_starts: usize,
    liveness: Option<Arc<crate::host::probes::Liveness>>,
    stop_host: bool,
    /// How long registration may wait for the broker to echo its marker.
    startup_timeout: Duration,
}

impl Default for HostControlLoopOptions {
    fn default() -> Self {
        Self {
            handler: None,
            host_group: None,
            heartbeat_interval: HEARTBEAT_INTERVAL,
            cleanup_interval: CLEANUP_INTERVAL,
            cleanup_age: CLEANUP_AGE,
            max_concurrent_starts: default_max_concurrent_starts(),
            liveness: None,
            stop_host: false,
            startup_timeout: CONTROL_STARTUP_TIMEOUT,
        }
    }
}

impl HostControlLoopOptions {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.max_concurrent_starts <= tokio::sync::Semaphore::MAX_PERMITS,
            "max_concurrent_starts exceeds the semaphore limit"
        );
        Ok(())
    }
}

struct OwnedHostStartup(Option<Arc<Host>>);

impl Drop for OwnedHostStartup {
    fn drop(&mut self) {
        if let Some(host) = self.0.take() {
            tokio::spawn(async move {
                if let Err(error) = host.stop().await {
                    error!(%error, "failed to stop host after cancelled control startup");
                }
            });
        }
    }
}

fn control_ready_subject(host_id: &str) -> String {
    rpc_subject(host_id, "__control.ready")
}

/// Observe our marker through the subscription being verified. SUB and PUB
/// share a connection, so receiving it proves the broker processed the SUB.
/// A rejected wildcard cannot be masked by a separate, permitted subscription.
async fn verify_subscription(
    subscription: &mut async_nats::Subscriber,
    nats_client: &async_nats::Client,
    host_id: &str,
) -> anyhow::Result<VecDeque<async_nats::Message>> {
    let subject = control_ready_subject(host_id);
    let marker = uuid::Uuid::new_v4().to_string();
    nats_client
        .publish(subject.clone(), marker.clone().into())
        .await
        .context("failed to publish host API registration marker")?;
    let mut pending = VecDeque::new();
    let mut shed = 0usize;
    while let Some(message) = subscription.next().await {
        if message.subject.as_str() == subject && message.payload.as_ref() == marker.as_bytes() {
            if shed > 0 {
                warn!(
                    shed,
                    "dropped commands that arrived during host API registration; \
                     their callers will retry"
                );
            }
            return Ok(pending);
        }
        // Shed the way the running loop does rather than fail the
        // registration: a busy control plane is no reason to refuse to attach.
        // Stops are kept, because nothing retries one that goes unanswered.
        if pending.len() < MAX_PENDING_REPLIES || command_name(&message) == "workload.stop" {
            pending.push_back(message);
        } else {
            shed += 1;
        }
    }
    anyhow::bail!("host API subscription closed during registration")
}

async fn control_barrier(host_id: &str, client: &async_nats::Client) -> anyhow::Result<()> {
    let mut subscription = client.subscribe(control_ready_subject(host_id)).await?;
    verify_subscription(&mut subscription, client, host_id).await?;
    subscription.unsubscribe().await?;
    client
        .flush()
        .await
        .context("failed to flush control barrier unsubscribe")
}

fn control_interval(interval: Duration) -> tokio::time::Interval {
    let mut timer = tokio::time::interval(interval);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    timer
}

async fn subscribe_host(
    host: &Host,
    nats_client: &async_nats::Client,
    timeout: Duration,
) -> anyhow::Result<(async_nats::Subscriber, VecDeque<async_nats::Message>)> {
    tokio::time::timeout(timeout, async {
        let mut subscription = nats_client
            .subscribe(host_subject(host.id()))
            .await
            .context("failed to subscribe for API requests")?;
        let pending = verify_subscription(&mut subscription, nats_client, host.id())
            .await
            .context("failed to verify host API subscription")?;
        Ok((subscription, pending))
    })
    .await
    // A broker that refuses the SUB or the marker says so on the connection,
    // not to this call, so a refusal looks the same from here as a marker that
    // never comes back. Name every cause the caller can act on.
    .with_context(|| {
        format!(
            "timed out registering host API subscription after {timeout:?}; check that the \
             NATS client is connected, may subscribe to `{}` and publish to `{}`, and was \
             not connected with `no_echo`",
            host_subject(host.id()),
            control_ready_subject(host.id()),
        )
    })?
}

fn spawn_control_loop(
    host: Arc<Host>,
    nats_client: Arc<async_nats::Client>,
    mut api_subscription: async_nats::Subscriber,
    mut pending: VecDeque<async_nats::Message>,
    control: Arc<crate::host::HostControlLease>,
    mut one_shot_rx: oneshot::Receiver<()>,
    options: HostControlLoopOptions,
) -> JoinHandle<anyhow::Result<()>> {
    let HostControlLoopOptions {
        handler,
        host_group,
        heartbeat_interval,
        cleanup_interval,
        cleanup_age,
        max_concurrent_starts,
        liveness,
        stop_host,
        startup_timeout: _,
    } = options;
    let host_id = host.id().to_string();
    tokio::task::spawn(async move {
        let heartbeat_subject = heartbeat_subject(&host_id);
        let mut heartbeat_timer = control_interval(heartbeat_interval);

        // Only `workload.start` waits on this permit; stops and status
        // never do, so neither queues behind a pull.
        let starts = Arc::new(tokio::sync::Semaphore::new(max_concurrent_starts));
        let defaults = HostControlDefaults {
            host: host.clone(),
            starts: starts.clone(),
            _control: Some(control),
        };
        // Commands run as their own tasks, so shutdown has to wait for
        // them before this loop or an attached host's owner stops the
        // host. `host.stop()` unbinds every plugin; a start running past
        // it would bind against stopped plugins.
        let mut commands = tokio::task::JoinSet::new();
        let mut background = tokio::task::JoinSet::new();
        let start_slots = Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_STARTS));
        let query_slots = Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_QUERIES));
        // One publisher and a bounded queue prevent disconnected NATS from
        // turning answered commands into unbounded reply tasks.
        let (replies, mut reply_rx) = tokio::sync::mpsc::channel::<ApiReply>(MAX_PENDING_REPLIES);
        let reply_client = Arc::clone(&nats_client);
        let mut publisher = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            while let Some((subject, bytes)) = reply_rx.recv().await {
                if let Err(error) = reply_client.publish(subject, bytes.into()).await {
                    error!(%error, "failed to publish API response");
                }
            }
        }));
        let mut publisher_finished = false;
        // Whether the last start or status found no room, so a run of shed
        // commands is reported once rather than once each.
        let mut shedding = false;

        // Read once. Nothing changes the host's config after it is
        // built, so a host with no cache directory has none for its
        // whole life and gets no timer at all rather than one that
        // wakes to reach the same answer. The path is behind an `Arc`
        // so each tick hands it to a task without copying it.
        let mut oci_cleanup = host
            .config()
            .oci_cache_dir
            .clone()
            .map(|dir| (Arc::new(dir), control_interval(cleanup_interval)));

        // Heartbeats and cache cleanup run as their own tasks, for the
        // reason commands already do: `select!` runs one branch to
        // completion, so anything awaited in one stops the loop turning
        // — and the loop turning is the whole of what `/livez` reports.
        //
        // A heartbeat publish ends in a send on async-nats' bounded
        // command channel, which stops draining while NATS is
        // unreachable. Awaited here, an outage would hold the loop past
        // the liveness budget and the kubelet would restart the host:
        // every workload on it lost, every host in the fleet at once
        // because they all watch the same NATS, and none of it a thing
        // a restart can fix. Cache cleanup is a filesystem walk, the
        // same shape with a slower fuse.
        //
        // One at a time each, and a tick that finds the previous still
        // running is dropped rather than queued: a heartbeat that has
        // not gone out is not improved by a second behind it, and two
        // in flight can arrive out of order. The permit lives in the
        // task, so it comes back however the task ends.
        let heartbeat_slot = Arc::new(tokio::sync::Semaphore::new(1));
        let cleanup_slot = Arc::new(tokio::sync::Semaphore::new(1));

        // Built once and pinned rather than rebuilt every turn, over an
        // owned handle so the shutdown after the loop can still consume
        // `host`.
        let http_handler = Arc::clone(&host.http_handler);
        let mut ingress_stopped = std::pin::pin!(async move { http_handler.stopped().await });

        let ended = loop {
            // Every turn, whichever branch woke it. The heartbeat timer
            // alone guarantees one per interval, so silence here means
            // the loop itself has stopped — which is what `/livez`
            // reports and the only thing worth a restart.
            if let Some(liveness) = &liveness {
                liveness.beat();
            }
            tokio::select! {
                biased;

                // Shutdown signal
                _ = &mut one_shot_rx => break Ended::Requested,

                // The accept loop returned and nothing restarts it, so
                // this host would hold every workload it was given,
                // keep heartbeating, and serve no HTTP for as long as
                // it runs. Stop an owned host or report the failure
                // to an attached host's owner.
                () = &mut ingress_stopped => break Ended::IngressStopped,

                ended = async {
                    // Fairly choose work even when a short timer is always ready.
                    // Shutdown and ingress failure retain priority in the outer select.
                    tokio::select! {
                        // Native starts own cancellation cleanup. A handler panic
                        // still ends control so its owner can recover custom effects.
                        Some(finished) = commands.join_next() => {
                            if let Err(e) = finished {
                                return Some(Ended::CommandPanicked(e));
                            }
                        }

                        Some(finished) = background.join_next() => {
                            if let Err(error) = finished {
                                error!(%error, "host control background task failed");
                            }
                        }

                        // OCI cache cleanup
                        cache_dir = next_cache_cleanup(&mut oci_cleanup) => {
                            if let Ok(slot) = Arc::clone(&cleanup_slot).try_acquire_owned() {
                                let age = cleanup_age;
                                background.spawn(async move {
                                    let _slot = slot;
                                    if let Err(e) = oci::cleanup_cache(&*cache_dir, age).await {
                                        error!("error during OCI cache cleanup: {e}");
                                    }
                                });
                            }
                        }

                        // Send heartbeat
                        _ = heartbeat_timer.tick() => {
                            // Dropped rather than queued when the last one is
                            // still going. Nothing here returns on failure:
                            // returning would drop `commands`, aborting
                            // in-flight starts after they have reserved their
                            // ids, and skip the `host.stop()` that unbinds
                            // their plugins. A missed heartbeat is worth none
                            // of that; the next tick tries again.
                            match Arc::clone(&heartbeat_slot).try_acquire_owned() {
                                Ok(slot) => {
                                    let host = host.clone();
                                    let nats_client = nats_client.clone();
                                    let subject = heartbeat_subject.clone();
                                    let host_group = host_group.clone();
                                    background.spawn(async move {
                                        let _slot = slot;
                                        match host_heartbeat(&host, host_group.as_deref()).await.and_then(|heartbeat| {
                                            serde_json::to_vec(&heartbeat).context("failed to serialize heartbeat")
                                        }) {
                                            Ok(heartbeat_bytes) => {
                                                if let Err(e) = nats_client
                                                    .publish(subject, heartbeat_bytes.into())
                                                    .await
                                                {
                                                    error!("failed to publish heartbeat: {e}");
                                                }
                                            }
                                            Err(e) => error!("failed to build heartbeat: {e}"),
                                        }
                                    });
                                }
                                // Every tick this reports is one the operator
                                // did not hear, which is what its unreachable
                                // window is for. Said out loud because the
                                // cause — a NATS that is not draining — is
                                // otherwise visible only as a host going quiet.
                                Err(_) => warn!(
                                    "previous heartbeat has not finished publishing; skipping this one"
                                ),
                            }
                        }

                        // Handle API requests
                        finished = &mut publisher => {
                            publisher_finished = true;
                            return Some(match finished {
                                Err(error) => Ended::CommandPanicked(error),
                                Ok(()) => Ended::ReplyPublisherStopped,
                            });
                        }

                        message = async {
                            match pending.pop_front() {
                                Some(message) => Some(message),
                                None => api_subscription.next().await,
                            }
                        } => {
                            let Some(msg) = message else {
                                return Some(Ended::SubscriptionClosed);
                            };
                            if command_name(&msg) == "__control.ready" {
                                return None;
                            }
                            // `select!` runs one branch to completion, so a
                            // command awaited here would hold up the heartbeat
                            // above for as long as it takes to pull and compile.
                            // A host whose heartbeats stop looks unreachable to
                            // the operator, which deletes it and its workloads.
                            //
                            // Commands naming one workload are ordered by the
                            // host's reservation on that id, not by this loop:
                            // a start claims the id before it fetches anything,
                            // and a stop that finds the claim hands the teardown
                            // back to the start holding it.
                            let command = command_name(&msg);
                            let slots = if command == "workload.start" {
                                &start_slots
                            } else {
                                &query_slots
                            };
                            let slot = match Arc::clone(slots).try_acquire_owned() {
                                Ok(slot) => {
                                    shedding = false;
                                    Some(slot)
                                }
                                // A stop always runs. Nothing retries one that
                                // was turned away, so shedding it would leave
                                // its workload running with nobody tracking it.
                                Err(_) if command == "workload.stop" => None,
                                // Shed without a reply, so the caller times out
                                // and retries. Any typed reply would be read as
                                // the workload's own state: an errored start
                                // naming an id this host never claimed, or a
                                // running workload reported as failed.
                                Err(_) => {
                                    // Once per episode at `warn`: a flood is
                                    // what fills the slots, and a line for each
                                    // command in it would bury the first.
                                    if shedding {
                                        debug!(subject = %msg.subject, "host control is busy; dropping command");
                                    } else {
                                        warn!(
                                            subject = %msg.subject,
                                            "host control is busy; dropping commands for their callers to retry"
                                        );
                                        shedding = true;
                                    }
                                    return None;
                                }
                            };
                            let replies = replies.clone();
                            let defaults = defaults.clone();
                            let handler = handler.clone();
                            let host_group = host_group.clone();
                            commands.spawn(async move {
                                match handle_command(&defaults, &msg, handler.as_deref(), host_group.as_deref()).await {
                                    Ok(resp_bytes) => {
                                        if let Some(reply_to) = msg.reply {
                                            if slot.is_some() {
                                                let _ = replies.send((reply_to, resp_bytes)).await;
                                            } else {
                                                // Without a slot nothing bounds
                                                // how many of these wait on a
                                                // full queue, so the reply goes
                                                // out now or not at all.
                                                let _ = replies.try_send((reply_to, resp_bytes));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("error handling command: {e}");
                                    }
                                }
                            });
                        }
                    }
                    None
                } => {
                    if let Some(ended) = ended {
                        break ended;
                    }
                }
            }
        };

        // `wash host` watches the same accept loop and asks for a
        // shutdown the moment it ends, so both branches above can be
        // ready at once. Shutdown has priority, but must still report a
        // failed ingress. Only checked on the `Requested` path, so
        // the future is never polled after it has already completed.
        let ended = match ended {
            Ended::Requested if ingress_stopped.as_mut().now_or_never().is_some() => {
                Ended::IngressStopped
            }
            ended => ended,
        };

        // Stop admitting native starts and periodic work; let replies drain.
        starts.close();
        drop(replies);
        background.abort_all();

        // Stop receiving API requests before draining accepted commands.
        let unsubscribed = tokio::time::timeout(CONTROL_IO_TIMEOUT, api_subscription.unsubscribe())
            .await
            .context("timed out unsubscribing from API requests")
            .and_then(|result| result.context("failed to unsubscribe from API requests"));

        // Let accepted commands finish and replies publish within the drain
        // budget. Queued native starts release their IDs now admission is closed.
        let mut command_failure = None;
        let mut publisher_failure = None;
        let drained = tokio::time::timeout(COMMAND_DRAIN_TIMEOUT, async {
            tokio::join!(
                async {
                    while let Some(finished) = commands.join_next().await {
                        if let Err(e) = finished {
                            error!("command task failed during shutdown: {e}");
                            command_failure = Some(e);
                        }
                    }
                },
                async { while background.join_next().await.is_some() {} },
                async {
                    if !publisher_finished {
                        if let Err(error) = (&mut publisher).await {
                            publisher_failure = Some(error);
                        }
                        publisher_finished = true;
                    }
                },
            );
        })
        .await;

        // Cancel tasks that exceeded the drain budget, then wait for them to unwind.
        let mut commands_still_running = false;
        if drained.is_err() {
            warn!(
                "commands still running after {COMMAND_DRAIN_TIMEOUT:?}; \
                         aborting them before control shutdown"
            );
            commands.abort_all();
            publisher.abort();

            // Await cancellation before stopping plugins that may still be binding.
            // Bound the wait because synchronous compilation can delay cancellation.
            let unwound = tokio::time::timeout(COMMAND_ABORT_TIMEOUT, async {
                tokio::join!(
                    async {
                        while let Some(finished) = commands.join_next().await {
                            // A panic while unwinding still matters: the task
                            // may hold a workload id it never released.
                            if let Err(e) = finished
                                && !e.is_cancelled()
                            {
                                error!("aborted command task failed: {e}");
                                command_failure = Some(e);
                            }
                        }
                    },
                    async { while background.join_next().await.is_some() {} },
                    async {
                        if !publisher_finished {
                            let _ = (&mut publisher).await;
                            publisher_finished = true;
                        }
                    }
                );
            })
            .await;

            // Remember tasks still running so attached control reports incomplete shutdown.
            if unwound.is_err() {
                commands_still_running = true;
                warn!(
                    "commands still unwinding {COMMAND_ABORT_TIMEOUT:?} after \
                             abort; returning without waiting longer"
                );
            }
        }

        // Wait for resource cleanup spawned by cancelled native starts.
        let recovered = tokio::time::timeout(
            crate::timeouts::plugin_stop() + Duration::from_secs(1),
            host.wait_for_workload_start_cleanup(),
        )
        .await
        .context("timed out cleaning up cancelled workload starts")
        .and_then(std::convert::identity);

        // A marker echoed by the server confirms processing of UNSUB and
        // earlier replies; Client::flush alone only writes to the socket.
        let flushed =
            tokio::time::timeout(CONTROL_IO_TIMEOUT, control_barrier(&host_id, &nats_client))
                .await
                .context("timed out flushing host control shutdown")
                .and_then(|result| result.context("failed to flush host control shutdown"));

        // Stop the host only when control owns its lifecycle.
        let stopped = if stop_host {
            host.stop().await.context("failed to stop host")
        } else {
            Ok(())
        };

        // Everything shutdown left unfinished, in the order it limits what the
        // host's owner can do next: work that may still be running, cleanup
        // that needs a retry, then the control plane. All of it is reported
        // together, so a broker outage cannot hide a command still unwinding.
        let mut failures = Vec::new();
        if commands_still_running {
            failures.push("command tasks did not unwind after abort".to_string());
        } else if drained.is_err() {
            failures.push(
                "host control tasks exceeded the shutdown drain and were aborted".to_string(),
            );
        }
        if let Some(error) = command_failure {
            failures.push(format!("command task failed during shutdown: {error}"));
        }
        if let Some(error) = publisher_failure {
            failures.push(format!("reply publisher failed during shutdown: {error}"));
        }
        for result in [recovered, unsubscribed, flushed] {
            if let Err(error) = result {
                failures.push(format!("{error:#}"));
            }
        }
        let incomplete = (!failures.is_empty()).then(|| failures.join("; "));

        // Only a requested shutdown of an attached host returns these to its
        // owner; every other exit reports something else, so say them here.
        if let Some(incomplete) = &incomplete
            && (stop_host || !matches!(ended, Ended::Requested))
        {
            warn!(reason = %incomplete, "host control shutdown was incomplete");
        }

        // Report the exit reason and shutdown failures according to host ownership.
        match ended {
            Ended::Requested => {
                stopped?;
                match incomplete {
                    // Attached hosts stay running; surface failures to their owner.
                    Some(incomplete) if !stop_host => Err(anyhow::Error::msg(incomplete)),
                    // The owned host stopped cleanly; control-plane errors
                    // during shutdown do not fail the exit.
                    _ => Ok(()),
                }
            }
            Ended::ReplyPublisherStopped => {
                stopped?;
                Err(anyhow!("host API reply publisher stopped"))
            }
            Ended::IngressStopped => {
                // An owned host is stopped first. An attached host's
                // owner receives the error and controls recovery.
                stopped?;
                Err(anyhow!(
                    "HTTP ingress stopped accepting connections; \
                             the host can no longer serve traffic"
                ))
            }
            Ended::CommandPanicked(e) => {
                stopped?;
                Err(anyhow!("command task failed: {e}"))
            }
            Ended::SubscriptionClosed => {
                stopped?;
                Err(anyhow!("host API subscription closed"))
            }
        }
    })
}

impl ClusterHost {
    pub fn host(&self) -> &Host {
        &self.prepared_host
    }

    /// Start the host and verify its control subscription before returning.
    /// The NATS client needs the permissions documented by
    /// [`AttachedHostControlBuilder::attach`], along with heartbeat and reply
    /// publishing. Failed or cancelled registration tears down the started host.
    pub async fn start(
        self,
    ) -> anyhow::Result<(impl HostApi, impl Future<Output = anyhow::Result<()>>)> {
        let (one_shot_tx, one_shot_rx) = oneshot::channel();
        let nats_client = self.nats_client.clone();
        let host = self
            .prepared_host
            .start()
            .await
            .context("failed to start host")?;

        let heartbeat_interval = self.heartbeat_interval;
        let cleanup_interval = self.cleanup_interval;
        let max_concurrent_starts = self.max_concurrent_starts;
        let liveness = self.liveness.clone();
        let host_id = host.id().to_string();

        info!(
        host_id=?host_id,
        friendly_name=?host.friendly_name(),
        host_name=?host.hostname(),
        labels=?host.labels(),
        version=?host.version(),
        max_concurrent_starts,
        "Host started");

        host.log_interfaces();

        let mut startup = OwnedHostStartup(Some(host.clone()));
        let control = host.acquire_control()?;
        let options = HostControlLoopOptions {
            heartbeat_interval,
            cleanup_interval,
            cleanup_age: self.cleanup_age,
            max_concurrent_starts,
            liveness,
            stop_host: true,
            ..HostControlLoopOptions::default()
        };
        options.validate()?;
        let registered = subscribe_host(&host, &nats_client, options.startup_timeout).await;
        let (subscription, pending) = match registered {
            Ok(subscription) => subscription,
            Err(error) => {
                if let Err(stop_error) = host.clone().stop().await {
                    warn!(%stop_error, "failed to stop host after control subscription failed");
                }
                startup.0 = None;
                return Err(error);
            }
        };
        let task = spawn_control_loop(
            host.clone(),
            nats_client,
            subscription,
            pending,
            control,
            one_shot_rx,
            options,
        );
        startup.0 = None;

        Ok((host, async move {
            let _ = one_shot_tx.send(());
            task.await?
        }))
    }

    /// Run the cluster host, with no API access
    pub async fn run(self) -> anyhow::Result<impl Future<Output = anyhow::Result<()>>> {
        let (_host, cleanup) = self.start().await?;
        Ok(cleanup)
    }
}

pub async fn run_cluster_host(
    cluster_host: ClusterHost,
) -> anyhow::Result<impl Future<Output = anyhow::Result<()>>> {
    cluster_host.run().await
}

/// Configuration options for NATS connections
#[derive(Debug, Clone, Default)]
pub struct NatsConnectionOptions {
    /// Request timeout for NATS operations
    pub request_timeout: Option<Duration>,
    /// Path to TLS CA certificate file for NATS connection
    pub tls_ca: Option<PathBuf>,
    /// Enable TLS handshake first mode for NATS connection
    pub tls_first: bool,
    /// Path to NATS TLS certificate file
    pub tls_cert: Option<PathBuf>,
    /// Path to NATS TLS private key file
    pub tls_key: Option<PathBuf>,
    /// How long to keep retrying a refused *initial* connection before giving
    /// up. `None` gives up on the first refusal.
    ///
    /// Only the first connection needs this: once established, async-nats
    /// reconnects on its own and buffers through the gap. A host deployed
    /// beside its NATS has no ordering guarantee between the two, so without a
    /// window here it exits, and the pod restarts until NATS happens to be up
    /// first — which is a slower, noisier way to wait, and it spends the
    /// restart count that would otherwise mean something.
    ///
    /// Left unset by a command run against a NATS the operator already has up
    /// (`wash dev`), where a refusal is the answer rather than a race.
    pub connect_retry: Option<Duration>,
}

#[instrument(skip_all)]
pub async fn connect_nats(
    addr: impl async_nats::ToServerAddrs,
    options: NatsConnectionOptions,
) -> Result<async_nats::Client, anyhow::Error> {
    let mut opts = async_nats::ConnectOptions::new();

    if let Some(timeout) = options.request_timeout {
        opts = opts.request_timeout(Some(timeout));
    }

    if let Some(ca_path) = options.tls_ca {
        opts = opts.add_root_certificates(ca_path)
    }

    if options.tls_first {
        opts = opts.tls_first();
    }

    if let (Some(cert_path), Some(key_path)) = (options.tls_cert, options.tls_key) {
        opts = opts.add_client_certificate(cert_path, key_path)
    }

    // Without a callback these events are raised and discarded. `SlowConsumer`
    // in particular is the *only* signal that a subscription's buffer
    // overflowed and messages were dropped — async-nats `try_send`s into that
    // buffer and drops silently on overflow, so an unobserved event means
    // core-NATS traffic disappearing from a host that otherwise looks healthy.
    opts = opts.event_callback(|event| async move {
        match event {
            async_nats::Event::SlowConsumer(sid) => tracing::warn!(
                subscription = sid,
                "NATS slow consumer: the subscription buffer overflowed and messages were \
                 dropped. The handler is not keeping up — check for saturated messaging \
                 admission (`messaging.admission.shed`) or slow handlers"
            ),
            async_nats::Event::Disconnected => {
                tracing::warn!("disconnected from NATS; buffered operations will be retried")
            }
            async_nats::Event::Connected => tracing::info!("connected to NATS"),
            async_nats::Event::ClientError(err) => tracing::warn!(%err, "NATS client error"),
            async_nats::Event::ServerError(err) => tracing::warn!(%err, "NATS server error"),
            other => tracing::debug!(event = %other, "NATS connection event"),
        }
    });

    let Some(window) = options.connect_retry else {
        return opts
            .connect(addr)
            .await
            .context("failed to connect to NATS");
    };

    // `to_server_addrs` here rather than per attempt: the addresses are what
    // the retry is over, and resolving them once means a malformed URL is
    // reported as one instead of being retried for the whole window.
    let addrs = addr
        .to_server_addrs()
        .context("failed to parse NATS server address")?
        .collect::<Vec<_>>();

    let deadline = tokio::time::Instant::now() + window;
    let mut backoff = Duration::from_millis(250);
    loop {
        let err = match opts.clone().connect(addrs.clone()).await {
            Ok(client) => return Ok(client),
            Err(err) => err,
        };
        // The window is for a server that is not up yet. A credential the
        // server refuses, or a TLS setup it will not accept, reads the same on
        // the last attempt as the first — waiting it out turns an accurate
        // error into a minute of silence followed by that same error, with the
        // host looking like it is still starting.
        use async_nats::ConnectErrorKind;
        if !matches!(
            err.kind(),
            ConnectErrorKind::Io | ConnectErrorKind::TimedOut | ConnectErrorKind::Dns
        ) {
            return Err(anyhow::Error::new(err)).context("failed to connect to NATS");
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(anyhow::Error::new(err))
                .with_context(|| format!("failed to connect to NATS within {window:?}"));
        }
        tracing::warn!(%err, retry_in = ?backoff, "NATS is not accepting connections yet");
        tokio::time::sleep(backoff.min(remaining)).await;
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

/// The directory to clean when the next cleanup falls due, or never for a host
/// with no cache directory.
///
/// A `select!` branch needs a future whether or not there is anything to wait
/// for. This gives the branch one that simply never completes, which is what a
/// host with nothing to clean should do, rather than arming a timer and testing
/// a condition that was settled before the loop began.
async fn next_cache_cleanup(
    cleanup: &mut Option<(Arc<PathBuf>, tokio::time::Interval)>,
) -> Arc<PathBuf> {
    let Some((cache_dir, timer)) = cleanup else {
        return std::future::pending().await;
    };
    timer.tick().await;
    Arc::clone(cache_dir)
}

pub fn host_subject(host_id: &str) -> String {
    format!("{HOST_API_PREFIX}.{host_id}.>")
}

pub fn rpc_subject(host_id: &str, command: &str) -> String {
    format!("{HOST_API_PREFIX}.{host_id}.{command}")
}

pub fn heartbeat_subject(host_id: &str) -> String {
    format!("{OPERATOR_API_PREFIX}.heartbeat.{host_id}")
}

/// Helper function to serialize a message to the API format.
fn to_api<T: prost::Message + serde::Serialize>(msg: &T) -> Result<Vec<u8>, anyhow::Error> {
    serde_json::to_vec(msg).map_err(anyhow::Error::new)
}

/// Helper function to deserialize a message from the API format.
fn from_api<'de, T: serde::Deserialize<'de>>(bytes: &'de [u8]) -> Result<T, anyhow::Error> {
    serde_json::from_slice(bytes).map_err(anyhow::Error::new)
}

type ApiReply = (async_nats::Subject, Vec<u8>);

fn command_name(msg: &async_nats::Message) -> &str {
    msg.subject.splitn(4, '.').nth(3).unwrap_or_default()
}

#[instrument(level = "debug", skip_all, fields(subject = %msg.subject))]
async fn handle_command(
    defaults: &HostControlDefaults,
    msg: &async_nats::Message,
    handler: Option<&dyn HostCommandHandler>,
    host_group: Option<&str>,
) -> Result<Vec<u8>, anyhow::Error> {
    let command = command_name(msg);
    let payload = &msg.payload;
    match command {
        "heartbeat" => to_api(&host_heartbeat(&defaults.host, host_group).await?),
        "workload.start" => {
            let req: types::v2::WorkloadStartRequest = match from_api(payload) {
                Ok(request) => request,
                Err(error) => {
                    return to_api(&workload_start_error(
                        "",
                        format!("invalid request: {error:#}"),
                    ));
                }
            };
            let workload_id = req.workload_id.clone();
            let response = match handler {
                Some(handler) => handler.start(defaults, req).await,
                None => defaults.start(req).await,
            };
            to_api(
                &response.unwrap_or_else(|error| {
                    workload_start_error(&workload_id, format!("{error:#}"))
                }),
            )
        }
        "workload.stop" => {
            let req: types::v2::WorkloadStopRequest = match from_api(payload) {
                Ok(request) => request,
                Err(error) => {
                    return to_api(&types::v2::WorkloadStopResponse {
                        workload_status: Some(command_error(
                            "",
                            format!("invalid request: {error:#}"),
                        )),
                    });
                }
            };
            let workload_id = req.workload_id.clone();
            let response = match handler {
                Some(handler) => handler.stop(defaults, req).await,
                None => defaults.stop(req).await,
            };
            to_api(
                &response.unwrap_or_else(|error| types::v2::WorkloadStopResponse {
                    workload_status: Some(command_error(&workload_id, format!("{error:#}"))),
                }),
            )
        }
        "workload.status" => {
            let req: types::v2::WorkloadStatusRequest = match from_api(payload) {
                Ok(request) => request,
                Err(error) => {
                    return to_api(&types::v2::WorkloadStatusResponse {
                        workload_status: Some(command_error(
                            "",
                            format!("invalid request: {error:#}"),
                        )),
                    });
                }
            };
            let workload_id = req.workload_id.clone();
            let response = match handler {
                Some(handler) => handler.status(defaults, req).await,
                None => defaults.status(req).await,
            };
            to_api(
                &response.unwrap_or_else(|error| types::v2::WorkloadStatusResponse {
                    workload_status: Some(command_error(&workload_id, format!("{error:#}"))),
                }),
            )
        }
        _ => anyhow::bail!("unknown command: {command}"),
    }
}

/// Convert ImagePullSecret from protobuf to OciConfig
fn image_pull_secret_to_oci_config(
    config: &HostConfig,
    pull_secret: &Option<types::v2::ImagePullSecret>,
) -> oci::OciConfig {
    let mut oci_config = match &pull_secret {
        Some(creds) => oci::OciConfig::new_with_credentials(&creds.username, &creds.password),
        None => OciConfig::default(),
    };
    oci_config.cache_dir = config.oci_cache_dir.clone();
    oci_config.insecure = config.allow_oci_insecure;
    oci_config.timeout = config.oci_pull_timeout;

    oci_config
}

/// Build the runtime's component from the one on the wire, once its image has
/// been pulled and its resources parsed.
///
/// Every field the wire carries has to land here: one dropped in this
/// conversion is unreachable from a deployed workload while looking wired
/// everywhere else. Split out from the pull loop so that stays true under test
/// without an image pull. The instance limits travel verbatim — the runtime
/// decodes them once, in [`crate::engine::InstancePolicy`].
fn component_from_wire(
    wire: &types::v2::Component,
    loaded: LoadedComponent,
    local_resources: crate::types::LocalResources,
) -> crate::types::Component {
    crate::types::Component {
        name: wire.name.clone(),
        bytes: loaded.bytes,
        digest: loaded.digest,
        local_resources,
        pool_size: wire.pool_size,
        max_invocations: wire.max_invocations,
        max_concurrency: wire.max_concurrency,
        reclaim_window_seconds: wire.reclaim_window_seconds,
        reclaim_min_instances: wire.reclaim_min_instances,
    }
}

#[instrument(level = "debug", skip_all)]
async fn host_heartbeat(
    host: &impl HostApi,
    host_group: Option<&str>,
) -> anyhow::Result<types::v2::HostHeartbeat> {
    let mut hb = host.heartbeat().await?;
    if let Some(host_group) = host_group {
        hb.labels.insert("hostgroup".into(), host_group.into());
    }

    Ok(hb.into())
}

/// A workload that could not be started, reported to the scheduler and to this
/// host's own log.
///
/// The response travels back to whoever asked, and nothing else here would say
/// why: an image the host cannot pull — a registry behind a CA it does not
/// trust, a reference that does not exist — otherwise leaves no trace on the
/// machine that failed to pull it.
fn workload_start_error(workload_id: &str, message: String) -> types::v2::WorkloadStartResponse {
    // `reason`, not `message`: `message` is the field tracing gives the event's
    // own text, and a second one under that name displaces it.
    error!(workload_id, reason = message, "failed to start workload");
    types::v2::WorkloadStartResponse {
        workload_status: Some(command_error(workload_id, message)),
    }
}

fn command_error(workload_id: &str, message: String) -> types::v2::WorkloadStatus {
    types::v2::WorkloadStatus {
        workload_id: workload_id.to_string(),
        workload_state: types::v2::WorkloadState::Error.into(),
        message,
    }
}
#[instrument(skip_all, fields(
    workload_id = %req.workload_id,
    workload.name=?req.workload.as_ref().map(|w| &w.name).unwrap_or(&"<none>".to_string()),
    workload.namespace=?req.workload.as_ref().map(|w| &w.namespace).unwrap_or(&"<none>".to_string())),
    )]
async fn workload_start(
    host: &Host,
    req: types::v2::WorkloadStartRequest,
    config: &HostConfig,
    starts: &tokio::sync::Semaphore,
) -> anyhow::Result<types::v2::WorkloadStartResponse> {
    let Some(types::v2::Workload {
        namespace,
        name,
        annotations,
        service,
        wit_world,
        volumes,
    }) = req.workload
    else {
        anyhow::bail!("workload is required");
    };

    let workload_id = req.workload_id.clone();
    if workload_id.is_empty() {
        anyhow::bail!("workload_id is required");
    }

    // Claimed before any image is fetched. Until the host holds the id a
    // status reports it missing and a stop reports it already gone, so a stop
    // arriving here would tell the operator the teardown is done while this
    // start goes on to run the workload.
    let reservation = match host.workload_reserve(&workload_id).await {
        Ok(reservation) => reservation,
        // Reported, not logged again: the host logs this refusal at `warn`
        // precisely because a scheduler replaying a start is not a malfunction,
        // and `workload_start_error` would raise the same event to `error`.
        Err(message) => {
            return Ok(types::v2::WorkloadStartResponse {
                workload_status: Some(types::v2::WorkloadStatus {
                    workload_id,
                    workload_state: types::v2::WorkloadState::Error.into(),
                    message,
                }),
            });
        }
    };

    let mut start_guard = host.workload_start_guard(&workload_id, reservation);
    // Queued with the id already claimed. Waiting for a permit is time like
    // any other in which a stop or a status has to find this workload.
    let _permit = match starts.acquire().await {
        Ok(permit) => permit,
        Err(e) => {
            host.workload_release(&workload_id, reservation).await;
            start_guard.disarm();
            return Ok(workload_start_error(
                &workload_id,
                format!("host is no longer accepting starts: {e}"),
            ));
        }
    };

    // The guard retains the reservation until cancellation cleanup finishes.
    let prepared = async {
        let (components, host_interfaces) = if let Some(wit_world) = wit_world {
            let mut pulled_components = Vec::with_capacity(wit_world.components.len());
            for component in &wit_world.components {
                let oci_config =
                    image_pull_secret_to_oci_config(config, &component.image_pull_secret);
                let source = ComponentSource::Oci {
                    image: component.image.clone(),
                    pull_policy: component.image_pull_policy().into(),
                };
                // `load` already names the reference it failed on; this says which
                // of the workload's components asked for it, so a multi-component
                // start reports something the operator can act on.
                let loaded = match source.load(oci_config).await.with_context(|| {
                    format!("failed to pull image for component '{}'", component.name)
                }) {
                    Ok(loaded) => loaded,
                    Err(e) => return Err(format!("{e:#}")),
                };
                let local_resources = match component.local_resources.clone() {
                    Some(lr) => match crate::types::LocalResources::try_from(lr) {
                        Ok(lr) => lr,
                        Err(e) => {
                            return Err(format!(
                                "invalid local_resources for component {}: {e:#}",
                                component.name
                            ));
                        }
                    },
                    None => crate::types::LocalResources::default(),
                };
                pulled_components.push(component_from_wire(component, loaded, local_resources))
            }
            (
                pulled_components,
                wit_world
                    .host_interfaces
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            )
        } else {
            (vec![], vec![])
        };

        let service = if let Some(service) = service {
            let oci_config = image_pull_secret_to_oci_config(config, &service.image_pull_secret);
            let source = ComponentSource::Oci {
                image: service.image.clone(),
                pull_policy: service.image_pull_policy().into(),
            };
            // Distinguishes a service pull failure from a component one; both
            // otherwise report the same reference and cause.
            let loaded = match source
                .load(oci_config)
                .await
                .context("failed to pull image for the workload service")
            {
                Ok(loaded) => loaded,
                Err(e) => return Err(format!("{e:#}")),
            };
            let local_resources = match service.local_resources.clone() {
                Some(lr) => match crate::types::LocalResources::try_from(lr) {
                    Ok(lr) => lr,
                    Err(e) => {
                        return Err(format!("invalid local_resources for service: {e:#}"));
                    }
                },
                None => crate::types::LocalResources::default(),
            };
            Some(crate::types::Service {
                bytes: loaded.bytes,
                digest: loaded.digest,
                local_resources,
                max_restarts: service.max_restarts,
            })
        } else {
            None
        };

        let volumes = volumes.into_iter().map(Into::into).collect();

        let request = crate::types::WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: crate::types::Workload {
                namespace,
                name,
                annotations,
                service,
                components,
                host_interfaces,
                volumes,
            },
        };
        Ok(request)
    }
    .await;

    let request = match prepared {
        Ok(request) => request,
        Err(message) => {
            host.workload_release(&workload_id, reservation).await;
            start_guard.disarm();
            return Ok(workload_start_error(&workload_id, message));
        }
    };

    info!(
        workload_id=?workload_id,
        namespace=?request.workload.namespace,
        name=?request.workload.name,
        "Starting workload");

    // Transfer cleanup ownership to the native start, with no intervening await.
    start_guard.disarm();
    Ok(host
        .workload_start_reserved(reservation, request)
        .await?
        .into())
}

#[instrument(skip_all, fields(workload_id = %req.workload_id))]
async fn workload_stop(
    host: &impl HostApi,
    req: types::v2::WorkloadStopRequest,
) -> anyhow::Result<types::v2::WorkloadStopResponse> {
    info!(
        workload_id=?req.workload_id,
        "Stopping workload");

    host.workload_stop(req.into()).await.map(|resp| resp.into())
}

#[instrument(skip_all, fields(workload_id = %req.workload_id))]
async fn workload_status(
    host: &impl HostApi,
    req: types::v2::WorkloadStatusRequest,
) -> anyhow::Result<types::v2::WorkloadStatusResponse> {
    debug!(
        workload_id=?req.workload_id,
        "Fetching workload status");

    host.workload_status(req.into())
        .await
        .map(|resp| resp.into())
}

impl From<types::v2::WitInterface> for crate::wit::WitInterface {
    fn from(wi: types::v2::WitInterface) -> Self {
        crate::wit::WitInterface {
            namespace: wi.namespace,
            package: wi.package,
            version: if wi.version.is_empty() {
                None
            } else {
                wi.version.parse::<semver::Version>().ok()
            },
            interfaces: wi.interfaces.into_iter().collect(),
            config: wi.config,
            name: if wi.name.is_empty() {
                None
            } else {
                Some(wi.name)
            },
        }
    }
}
impl From<types::v2::VolumeMount> for crate::types::VolumeMount {
    fn from(vm: types::v2::VolumeMount) -> Self {
        crate::types::VolumeMount {
            name: vm.name,
            mount_path: vm.mount_path,
            read_only: vm.read_only,
        }
    }
}

impl From<types::v2::Volume> for crate::types::Volume {
    fn from(v: types::v2::Volume) -> Self {
        crate::types::Volume {
            name: v.name,
            volume_type: match v.volume_type {
                Some(vt) => match vt {
                    types::v2::volume::VolumeType::HostPath(hp) => {
                        crate::types::VolumeType::HostPath(crate::types::HostPathVolume {
                            local_path: hp.local_path,
                        })
                    }
                    types::v2::volume::VolumeType::EmptyDir(_) => {
                        crate::types::VolumeType::EmptyDir(crate::types::EmptyDirVolume {})
                    }
                },
                None => crate::types::VolumeType::EmptyDir(crate::types::EmptyDirVolume {}),
            },
        }
    }
}

impl TryFrom<types::v2::LocalResources> for crate::types::LocalResources {
    type Error = anyhow::Error;

    fn try_from(lr: types::v2::LocalResources) -> Result<Self, Self::Error> {
        Ok(crate::types::LocalResources {
            memory_limit_mb: lr.memory_limit_mb,
            cpu_limit: lr.cpu_limit,
            config: lr.config,
            volume_mounts: lr.volume_mounts.into_iter().map(Into::into).collect(),
            allowed_hosts: parse_policy_entries(&lr.allowed_hosts, "allowed_hosts")?,
            environment: lr.environment,
            allowed_ip_name_lookups: parse_policy_entries(
                &lr.allowed_ip_name_lookups,
                "allowed_ip_name_lookups",
            )?,
            allowed_host_loopback_ports: parse_policy_entries(
                &lr.allowed_host_loopback_ports,
                "allowed_host_loopback_ports",
            )?,
        })
    }
}

/// Parses each entry of a policy list arriving from the wire, reporting
/// every bad entry at once.
///
/// A malformed entry fails the conversion so the workload start surfaces a
/// clear error rather than silently widening what the component may reach.
/// Failures are collected and joined into one message, rendered as a
/// heading line plus a bullet per bad entry, so a workload with several bad
/// entries doesn't have to be fixed one at a time.
fn parse_policy_entries<T>(entries: &[String], field: &str) -> anyhow::Result<Arc<[T]>>
where
    T: std::str::FromStr<Err = anyhow::Error>,
{
    let mut parsed: Vec<T> = Vec::with_capacity(entries.len());
    let mut errors: Vec<String> = Vec::new();
    for entry in entries {
        match entry.parse::<T>() {
            Ok(value) => parsed.push(value),
            Err(e) => errors.push(format!("'{entry}': {e:#}")),
        }
    }
    if !errors.is_empty() {
        return Err(anyhow!("invalid {field}:\n  - {}", errors.join("\n  - ")));
    }
    Ok(parsed.into())
}

impl From<crate::types::HostHeartbeat> for types::v2::HostHeartbeat {
    fn from(hb: crate::types::HostHeartbeat) -> Self {
        types::v2::HostHeartbeat {
            id: hb.id,
            hostname: hb.hostname,
            version: hb.version,
            started_at: Some(hb.started_at.into()),
            imports: hb.imports.into_iter().map(Into::into).collect(),
            exports: hb.exports.into_iter().map(Into::into).collect(),
            os_name: hb.os_name,
            os_arch: hb.os_arch,
            os_kernel: hb.os_kernel,
            system_cpu_usage: hb.system_cpu_usage,
            component_count: hb.component_count,
            workload_count: hb.workload_count,
            system_memory_total: hb.system_memory_total,
            system_memory_free: hb.system_memory_free,
            labels: hb.labels,
            friendly_name: hb.friendly_name,
            http_port: hb.http_port.into(),
            environment: hb.environment,
        }
    }
}

impl From<crate::wit::WitInterface> for types::v2::WitInterface {
    fn from(wi: crate::wit::WitInterface) -> Self {
        types::v2::WitInterface {
            namespace: wi.namespace,
            package: wi.package,
            version: wi.version.map(|v| v.to_string()).unwrap_or_default(),
            interfaces: wi.interfaces.into_iter().collect(),
            config: wi.config,
            name: wi.name.unwrap_or_default(),
        }
    }
}

// Conversions from API v2 request types to runtime::host types

impl From<types::v2::WorkloadStopRequest> for crate::types::WorkloadStopRequest {
    fn from(req: types::v2::WorkloadStopRequest) -> Self {
        crate::types::WorkloadStopRequest {
            workload_id: req.workload_id,
        }
    }
}

impl From<types::v2::WorkloadStatusRequest> for crate::types::WorkloadStatusRequest {
    fn from(req: types::v2::WorkloadStatusRequest) -> Self {
        crate::types::WorkloadStatusRequest {
            workload_id: req.workload_id,
        }
    }
}

// Conversions from runtime::host response types to API v2 types

impl From<crate::types::WorkloadStartResponse> for types::v2::WorkloadStartResponse {
    fn from(resp: crate::types::WorkloadStartResponse) -> Self {
        types::v2::WorkloadStartResponse {
            workload_status: Some(resp.workload_status.into()),
        }
    }
}

impl From<crate::types::WorkloadStopResponse> for types::v2::WorkloadStopResponse {
    fn from(resp: crate::types::WorkloadStopResponse) -> Self {
        types::v2::WorkloadStopResponse {
            workload_status: Some(resp.workload_status.into()),
        }
    }
}

impl From<crate::types::WorkloadStatusResponse> for types::v2::WorkloadStatusResponse {
    fn from(resp: crate::types::WorkloadStatusResponse) -> Self {
        types::v2::WorkloadStatusResponse {
            workload_status: Some(resp.workload_status.into()),
        }
    }
}

impl From<crate::types::WorkloadStatus> for types::v2::WorkloadStatus {
    fn from(status: crate::types::WorkloadStatus) -> Self {
        types::v2::WorkloadStatus {
            workload_id: status.workload_id,
            workload_state: status.workload_state as i32,
            message: status.message,
        }
    }
}

impl From<types::v2::ImagePullPolicy> for crate::oci::OciPullPolicy {
    fn from(policy: types::v2::ImagePullPolicy) -> Self {
        match policy {
            types::v2::ImagePullPolicy::Always => crate::oci::OciPullPolicy::Always,
            types::v2::ImagePullPolicy::IfNotPresent => crate::oci::OciPullPolicy::IfNotPresent,
            types::v2::ImagePullPolicy::Never => crate::oci::OciPullPolicy::Never,
            _ => crate::oci::OciPullPolicy::IfNotPresent,
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::host::allowed_hosts::AllowedHost;
    use crate::host::allowed_ip_name::AllowedIpName;

    #[tokio::test]
    async fn cancelled_and_concurrent_shutdowns_wait_for_the_same_task() -> anyhow::Result<()> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (observed_tx, observed_rx) = oneshot::channel();
        let (finish_tx, finish_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            shutdown_rx.await?;
            let _ = observed_tx.send(());
            finish_rx.await?;
            Ok(())
        });
        let channel = AttachedHostControl::new(shutdown_tx, task);
        // Poll and cancel the first wait after it has signalled shutdown.
        assert!(channel.shutdown().now_or_never().is_none());
        observed_rx.await?;
        let mut second = Box::pin(channel.shutdown());
        assert!(second.as_mut().now_or_never().is_none());
        assert!(channel.shutdown().now_or_never().is_none());
        let _ = finish_tx.send(());
        second.await?;
        channel.shutdown().await?;
        channel.stopped().await
    }

    // Paused, so the sixteen seconds below cost nothing: only timers are
    // waited on, and the clock skips to whichever is due first.
    #[tokio::test(start_paused = true)]
    async fn attachment_shutdown_honors_the_configured_plugin_cleanup_budget() -> anyhow::Result<()>
    {
        const CHILD: &str = "WASH_TEST_ATTACHMENT_SHUTDOWN_BUDGET_CHILD";
        if std::env::var(CHILD).as_deref() != Ok("1") {
            // Timeout accessors cache environment settings; use a fresh process
            // so this override cannot affect other tests running concurrently.
            let output = std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", "washlet::tests::attachment_shutdown_honors_the_configured_plugin_cleanup_budget", "--nocapture"])
                .env(CHILD, "1")
                .env("WASH_PLUGIN_STOP_TIMEOUT_SECS", "20")
                .output()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            // A filter that matches nothing also exits successfully, so a
            // renamed test or module would otherwise pass here unrun.
            anyhow::ensure!(
                output.status.success() && stdout.contains("1 passed"),
                "shutdown budget child test failed or did not run: {stdout} {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            shutdown_rx.await?;
            // Longer than the former fixed 15-second attachment timeout,
            // but within the configured native-start cleanup budget.
            tokio::time::sleep(Duration::from_secs(16)).await;
            Ok(())
        });
        let control = AttachedHostControl::new(shutdown_tx, task);
        control.shutdown().await?;
        control.stopped().await
    }

    struct NotifyOnDrop(Option<oneshot::Sender<()>>);

    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn dropping_after_a_cancelled_shutdown_still_aborts_the_task() -> anyhow::Result<()> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (observed_tx, observed_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _drop = NotifyOnDrop(Some(dropped_tx));
            shutdown_rx.await?;
            let _ = observed_tx.send(());
            std::future::pending::<()>().await;
            Ok(())
        });
        let channel = AttachedHostControl::new(shutdown_tx, task);
        assert!(channel.shutdown().now_or_never().is_none());
        observed_rx.await?;
        drop(channel);
        tokio::time::timeout(Duration::from_secs(1), dropped_rx).await??;
        Ok(())
    }

    #[tokio::test]
    async fn every_completion_observer_receives_the_exit_error() -> anyhow::Result<()> {
        let (shutdown_tx, _) = oneshot::channel();
        let task = tokio::spawn(async { Err(anyhow!("control subscription failed")) });
        let channel = AttachedHostControl::new(shutdown_tx, task);
        for result in [
            channel.stopped().await,
            channel.shutdown().await,
            channel.shutdown().await,
        ] {
            let error = result.err().context("exit failure was lost")?;
            assert!(error.to_string().contains("control subscription failed"));
        }
        Ok(())
    }

    fn command_message(command: &str, payload: Vec<u8>) -> async_nats::Message {
        async_nats::Message {
            subject: rpc_subject("test-host", command).into(),
            reply: None,
            length: payload.len(),
            payload: payload.into(),
            headers: None,
            status: None,
            description: None,
        }
    }

    fn control_defaults(host: Arc<Host>) -> HostControlDefaults {
        HostControlDefaults {
            host,
            starts: Arc::new(tokio::sync::Semaphore::new(1)),
            _control: None,
        }
    }

    struct RefusingCommands;

    #[async_trait]
    impl HostCommandHandler for RefusingCommands {
        async fn start(
            &self,
            _defaults: &HostControlDefaults,
            _request: types::v2::WorkloadStartRequest,
        ) -> anyhow::Result<types::v2::WorkloadStartResponse> {
            anyhow::bail!("admission denied");
        }

        async fn stop(
            &self,
            _defaults: &HostControlDefaults,
            _request: types::v2::WorkloadStopRequest,
        ) -> anyhow::Result<types::v2::WorkloadStopResponse> {
            anyhow::bail!("ownership denied");
        }

        async fn status(
            &self,
            _defaults: &HostControlDefaults,
            _request: types::v2::WorkloadStatusRequest,
        ) -> anyhow::Result<types::v2::WorkloadStatusResponse> {
            anyhow::bail!("status denied");
        }
    }

    #[tokio::test]
    async fn invalid_requests_and_handler_errors_have_typed_replies() -> anyhow::Result<()> {
        let host = crate::host::HostBuilder::default().build()?.start().await?;
        let defaults = control_defaults(host.clone());
        for command in ["workload.start", "workload.stop", "workload.status"] {
            for payload in [br#"{"workloadId":"denied"}"#.to_vec(), b"{".to_vec()] {
                let malformed = payload == b"{";
                let msg = command_message(command, payload);
                let reply = handle_command(&defaults, &msg, Some(&RefusingCommands), None).await?;
                let reply: serde_json::Value = serde_json::from_slice(&reply)?;
                let status = reply
                    .get("workloadStatus")
                    .context("missing typed status")?;
                assert_eq!(
                    status
                        .get("workloadState")
                        .and_then(serde_json::Value::as_str),
                    Some("WORKLOAD_STATE_ERROR")
                );
                let id = status
                    .get("workloadId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                assert_eq!(id, if malformed { "" } else { "denied" });
            }
        }
        let msg = command_message(
            "workload.start",
            br#"{"workloadId":"missing-body"}"#.to_vec(),
        );
        let reply: types::v2::WorkloadStartResponse =
            serde_json::from_slice(&handle_command(&defaults, &msg, None, None).await?)?;
        let status = reply
            .workload_status
            .context("missing default error reply")?;
        assert_eq!(status.workload_state(), types::v2::WorkloadState::Error);
        assert_eq!(status.workload_id, "missing-body");
        host.stop().await
    }

    struct BlockingStart {
        entered: std::sync::atomic::AtomicUsize,
        gate: tokio::sync::Semaphore,
    }

    #[async_trait]
    impl HostCommandHandler for BlockingStart {
        async fn start(
            &self,
            defaults: &HostControlDefaults,
            request: types::v2::WorkloadStartRequest,
        ) -> anyhow::Result<types::v2::WorkloadStartResponse> {
            self.entered
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _permit = self.gate.acquire().await?;
            defaults.start(request).await
        }
    }

    #[tokio::test]
    async fn custom_handlers_control_work_before_native_delegation() -> anyhow::Result<()> {
        let host = crate::host::HostBuilder::default().build()?.start().await?;
        let defaults = control_defaults(host.clone());
        let handler = BlockingStart {
            entered: std::sync::atomic::AtomicUsize::new(0),
            gate: tokio::sync::Semaphore::new(0),
        };
        let message = |id: &str| -> anyhow::Result<_> {
            Ok(command_message(
                "workload.start",
                serde_json::to_vec(&types::v2::WorkloadStartRequest {
                    workload_id: id.into(),
                    workload: Some(types::v2::Workload::default()),
                })?,
            ))
        };
        let first_msg = message("first")?;
        let second_msg = message("second")?;
        let mut first = Box::pin(handle_command(&defaults, &first_msg, Some(&handler), None));
        let mut second = Box::pin(handle_command(&defaults, &second_msg, Some(&handler), None));
        assert!(first.as_mut().now_or_never().is_none());
        assert!(second.as_mut().now_or_never().is_none());
        assert_eq!(handler.entered.load(std::sync::atomic::Ordering::SeqCst), 2);
        let status_msg = command_message(
            "workload.status",
            serde_json::to_vec(&types::v2::WorkloadStatusRequest {
                workload_id: "local".into(),
            })?,
        );
        let status: types::v2::WorkloadStatusResponse = serde_json::from_slice(
            &handle_command(&defaults, &status_msg, Some(&handler), None).await?,
        )?;
        assert_eq!(
            status
                .workload_status
                .context("missing status")?
                .workload_state(),
            types::v2::WorkloadState::NotFound
        );
        handler.gate.add_permits(2);
        for bytes in [first.await?, second.await?] {
            let response: types::v2::WorkloadStartResponse = serde_json::from_slice(&bytes)?;
            assert_eq!(
                response
                    .workload_status
                    .context("missing start status")?
                    .workload_state(),
                types::v2::WorkloadState::Running
            );
        }
        host.stop().await
    }

    struct AnnotatingStatus;

    #[async_trait]
    impl HostCommandHandler for AnnotatingStatus {
        async fn status(
            &self,
            defaults: &HostControlDefaults,
            request: types::v2::WorkloadStatusRequest,
        ) -> anyhow::Result<types::v2::WorkloadStatusResponse> {
            let mut response = defaults.status(request).await?;
            if let Some(status) = &mut response.workload_status {
                status.message = "checked by embedder".into();
            }
            Ok(response)
        }
    }

    #[tokio::test]
    async fn delegated_starts_reserve_before_waiting_and_honor_a_queued_stop() -> anyhow::Result<()>
    {
        let host = crate::host::HostBuilder::default().build()?.start().await?;
        let defaults = control_defaults(host.clone());
        let permit = defaults.starts.acquire().await?;
        let message = |id: &str| -> anyhow::Result<_> {
            Ok(command_message(
                "workload.start",
                serde_json::to_vec(&types::v2::WorkloadStartRequest {
                    workload_id: id.into(),
                    workload: Some(types::v2::Workload::default()),
                })?,
            ))
        };
        let first_msg = message("first")?;
        let second_msg = message("second")?;
        // A status-only override still delegates starts through the handler.
        let handler = AnnotatingStatus;
        let mut first = Box::pin(handle_command(&defaults, &first_msg, Some(&handler), None));
        let mut second = Box::pin(handle_command(&defaults, &second_msg, Some(&handler), None));
        assert!(first.as_mut().now_or_never().is_none());
        assert!(second.as_mut().now_or_never().is_none());
        for id in ["first", "second"] {
            let status = defaults
                .status(types::v2::WorkloadStatusRequest {
                    workload_id: id.into(),
                })
                .await?
                .workload_status
                .context("missing queued start status")?;
            assert_eq!(status.workload_state(), types::v2::WorkloadState::Starting);
            let stopped = handler
                .stop(
                    &defaults,
                    types::v2::WorkloadStopRequest {
                        workload_id: id.into(),
                    },
                )
                .await?
                .workload_status
                .context("missing queued stop status")?;
            assert_eq!(stopped.workload_state(), types::v2::WorkloadState::Stopping);
        }
        drop(permit);
        let (first, second) = tokio::join!(first, second);
        for bytes in [first?, second?] {
            let response: types::v2::WorkloadStartResponse = serde_json::from_slice(&bytes)?;
            let status = response.workload_status.context("missing start status")?;
            assert_eq!(status.workload_state(), types::v2::WorkloadState::Stopping);
            let final_status = defaults
                .status(types::v2::WorkloadStatusRequest {
                    workload_id: status.workload_id,
                })
                .await?
                .workload_status
                .context("missing final status")?;
            assert_eq!(
                final_status.workload_state(),
                types::v2::WorkloadState::NotFound
            );
        }
        host.stop().await
    }

    #[tokio::test]
    async fn custom_handler_can_call_native_default_and_modify_its_reply() -> anyhow::Result<()> {
        let host = crate::host::HostBuilder::default().build()?.start().await?;
        let defaults = HostControlDefaults {
            host: host.clone(),
            starts: Arc::new(tokio::sync::Semaphore::new(1)),
            _control: None,
        };
        let reply = AnnotatingStatus
            .status(
                &defaults,
                types::v2::WorkloadStatusRequest {
                    workload_id: "missing".into(),
                },
            )
            .await?;
        let status = reply
            .workload_status
            .ok_or_else(|| anyhow::anyhow!("missing native status"))?;
        assert_eq!(status.workload_state(), types::v2::WorkloadState::NotFound);
        assert_eq!(status.message, "checked by embedder");
        let unchanged_stop = AnnotatingStatus
            .stop(
                &defaults,
                types::v2::WorkloadStopRequest {
                    workload_id: "missing".into(),
                },
            )
            .await?;
        assert_eq!(
            unchanged_stop
                .workload_status
                .ok_or_else(|| anyhow::anyhow!("missing native stop status"))?
                .workload_state(),
            types::v2::WorkloadState::NotFound
        );
        host.stop().await
    }

    #[tokio::test]
    async fn requested_shutdown_handles_a_failed_flush_by_host_ownership() -> anyhow::Result<()> {
        use crate::host::http::{DevRouter, HostHandler as _, Ingress};
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

        crate::init_crypto();
        for stop_host in [true, false] {
            // A minimal NATS peer allows registration before disconnecting.
            // While the client reconnects, shutdown's flush cannot finish.
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let addr = listener.local_addr()?;
            let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await?;
                socket
                    .write_all(b"INFO {\"max_payload\":1048576}\r\n")
                    .await?;
                let (reader, mut writer) = socket.into_split();
                let mut lines = BufReader::new(reader).lines();
                while let Some(line) = lines.next_line().await? {
                    if line == "PING" {
                        writer.write_all(b"PONG\r\n").await?;
                    }
                }
                anyhow::Ok(())
            }));
            let client = Arc::new(async_nats::connect(format!("nats://{addr}")).await?);
            let ingress = Arc::new(
                Ingress::builder(DevRouter::default(), "127.0.0.1:0".parse()?)
                    .build()
                    .await?,
            );
            let host = crate::host::HostBuilder::default()
                .with_http_handler(ingress.clone())
                .build()?
                .start()
                .await?;
            let subscription = client.subscribe(host_subject(host.id())).await?;
            client.flush().await?;
            server.abort();
            let _ = server.await;
            tokio::time::timeout(Duration::from_secs(5), async {
                while client.connection_state() != async_nats::connection::State::Disconnected {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await?;
            let (tx, rx) = oneshot::channel();
            // Request shutdown before the loop starts so the test exercises
            // the requested exit rather than a subscription or ingress exit.
            let _ = tx.send(());
            let task = spawn_control_loop(
                host.clone(),
                client,
                subscription,
                VecDeque::new(),
                host.acquire_control()?,
                rx,
                HostControlLoopOptions {
                    stop_host,
                    ..HostControlLoopOptions::default()
                },
            );
            let result = tokio::time::timeout(Duration::from_secs(5), task).await??;
            if stop_host {
                result.context("control-plane failure failed a clean owned-host shutdown")?;
                tokio::time::timeout(Duration::from_secs(1), ingress.stopped()).await?;
            } else {
                let error = result
                    .err()
                    .context("attached shutdown lost the flush error")?;
                assert!(
                    error
                        .to_string()
                        .contains("timed out flushing host control shutdown")
                );
                assert!(ingress.stopped().now_or_never().is_none());
                host.stop().await?;
            }
        }
        Ok(())
    }

    /// Reject exactly the permission under test while handling the connection
    /// handshake. A denied SUB must not be mistaken for successful readiness.
    /// The `Notify` fires when the peer reads a SUB, which is how a test
    /// learns that registration has begun without guessing how long it takes.
    async fn restricted_nats_peer(
        deny_subscribe: bool,
    ) -> anyhow::Result<(
        Arc<async_nats::Client>,
        tokio_util::task::AbortOnDropHandle<anyhow::Result<()>>,
        Arc<tokio::sync::Notify>,
    )> {
        use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
        crate::init_crypto();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let subscribed = Arc::new(tokio::sync::Notify::new());
        let saw_subscribe = Arc::clone(&subscribed);
        let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            socket
                .write_all(b"INFO {\"max_payload\":1048576}\r\n")
                .await?;
            let (reader, mut writer) = socket.into_split();
            let mut reader = BufReader::new(reader);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await? == 0 {
                    break;
                }
                let parts: Vec<_> = line.split_whitespace().collect();
                match parts.first().copied() {
                    Some("PING") => writer.write_all(b"PONG\r\n").await?,
                    Some("SUB") => {
                        saw_subscribe.notify_one();
                        if deny_subscribe {
                            writer
                                .write_all(b"-ERR 'Permissions Violation for Subscription'\r\n")
                                .await?;
                        }
                    }
                    Some("PUB") => {
                        let size: usize = parts.last().context("missing payload size")?.parse()?;
                        let mut payload = vec![0; size + 2];
                        reader.read_exact(&mut payload).await?;
                        if !deny_subscribe {
                            writer
                                .write_all(b"-ERR 'Permissions Violation for Publish'\r\n")
                                .await?;
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }));
        let client = Arc::new(
            async_nats::ConnectOptions::new()
                .client_capacity(16)
                .connect(format!("nats://{addr}"))
                .await?,
        );
        Ok((client, server, subscribed))
    }

    #[tokio::test]
    async fn denied_control_permissions_fail_registration_and_release_the_lease()
    -> anyhow::Result<()> {
        for deny_subscribe in [true, false] {
            let (client, _server, _) = restricted_nats_peer(deny_subscribe).await?;
            let host = crate::host::HostBuilder::default().build()?.start().await?;
            let mut builder = AttachedHostControl::builder(host.clone(), client);
            // A refusal only ever shows as the marker not coming back, so the
            // default wait would be spent in full on each permission.
            builder.options.startup_timeout = Duration::from_millis(500);
            let error = builder
                .attach()
                .await
                .err()
                .context("registration succeeded with denied permissions")?;
            let error = format!("{error:#}");
            assert!(error.contains("timed out"));
            // Nothing else tells the caller a refused permission from a slow
            // broker, so the message has to name what to check.
            assert!(error.contains("__control.ready") && error.contains("no_echo"));
            assert!(host.acquire_control().is_ok());
            assert_eq!(host.heartbeat().await?.id, host.id());
            host.stop().await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_owned_registration_stops_the_started_ingress() -> anyhow::Result<()> {
        use crate::host::http::{DevRouter, HostHandler as _, Ingress};
        let (client, _server, subscribed) = restricted_nats_peer(true).await?;
        let ingress = Arc::new(
            Ingress::builder(DevRouter::default(), "127.0.0.1:0".parse()?)
                .build()
                .await?,
        );
        let cluster = ClusterHostBuilder::default()
            .with_host_group("cancelled-start")
            .with_nats_client(client)
            .with_http_handler(ingress.clone())
            .build()?;
        let start = tokio::spawn(cluster.start());
        // The host subscribes only once it has started, so the peer seeing the
        // SUB is the ingress being up and the start waiting on registration.
        tokio::time::timeout(Duration::from_secs(5), subscribed.notified()).await?;
        assert!(ingress.stopped().now_or_never().is_none());
        start.abort();
        assert!(start.await.is_err());
        tokio::time::timeout(Duration::from_secs(2), ingress.stopped()).await?;
        Ok(())
    }

    #[tokio::test]
    async fn periodic_control_work_skips_missed_ticks() {
        let mut interval = control_interval(Duration::from_secs(1));
        interval.tick().await;
        tokio::time::sleep(Duration::from_millis(2100)).await;
        interval.tick().await;
        assert!(interval.tick().now_or_never().is_none());
    }

    #[tokio::test]
    #[ignore = "requires Docker (NATS) or NATS_URL"]
    async fn registration_retains_requests_received_before_the_marker() -> anyhow::Result<()> {
        use testcontainers::{
            GenericImage,
            core::{IntoContainerPort as _, WaitFor},
            runners::AsyncRunner as _,
        };

        crate::init_crypto();
        let (_container, url) = match std::env::var("NATS_URL") {
            Ok(url) => (None, url),
            Err(_) => {
                let container = GenericImage::new("nats", "2.12.8-alpine")
                    .with_exposed_port(4222.tcp())
                    .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
                    .start()
                    .await
                    .context("failed to start NATS container")?;
                let port = container.get_host_port_ipv4(4222).await?;
                (Some(container), format!("nats://127.0.0.1:{port}"))
            }
        };
        let client = Arc::new(async_nats::connect(url).await?);
        let host = crate::host::HostBuilder::default().build()?.start().await?;
        let mut subscription = client.subscribe(host_subject(host.id())).await?;
        let inbox = client.new_inbox();
        let mut response = client.subscribe(inbox.clone()).await?;
        // Both SUBs, the real RPC, and the marker use one connection, fixing
        // their server order without relying on Client::flush or a delay.
        client
            .publish_with_reply(
                rpc_subject(host.id(), "heartbeat"),
                inbox,
                b"null".as_slice().into(),
            )
            .await?;
        let pending = tokio::time::timeout(
            Duration::from_secs(2),
            verify_subscription(&mut subscription, &client, host.id()),
        )
        .await??;
        assert_eq!(pending.len(), 1);
        let (tx, rx) = oneshot::channel();
        let control = AttachedHostControl::new(
            tx,
            spawn_control_loop(
                host.clone(),
                client,
                subscription,
                pending,
                host.acquire_control()?,
                rx,
                HostControlLoopOptions::default(),
            ),
        );
        let reply = tokio::time::timeout(Duration::from_secs(2), response.next())
            .await?
            .context("reply subscription closed")?;
        let heartbeat: types::v2::HostHeartbeat = from_api(&reply.payload)?;
        assert_eq!(heartbeat.id, host.id());
        control.shutdown().await?;
        host.stop().await
    }

    /// Counts what the loop let through: starts and statuses together, which
    /// it bounds, and stops apart, which it never sheds.
    #[derive(Default)]
    struct CountingReplies {
        accepted: std::sync::atomic::AtomicUsize,
        stops: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl HostCommandHandler for CountingReplies {
        async fn start(
            &self,
            _defaults: &HostControlDefaults,
            _request: types::v2::WorkloadStartRequest,
        ) -> anyhow::Result<types::v2::WorkloadStartResponse> {
            self.accepted
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            anyhow::bail!("test response")
        }
        async fn stop(
            &self,
            _defaults: &HostControlDefaults,
            _request: types::v2::WorkloadStopRequest,
        ) -> anyhow::Result<types::v2::WorkloadStopResponse> {
            self.stops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            anyhow::bail!("test response")
        }
        async fn status(
            &self,
            _defaults: &HostControlDefaults,
            _request: types::v2::WorkloadStatusRequest,
        ) -> anyhow::Result<types::v2::WorkloadStatusResponse> {
            self.accepted
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            anyhow::bail!("test response")
        }
    }

    #[tokio::test]
    async fn saturated_replies_during_an_outage_bound_tasks_and_keep_the_loop_alive()
    -> anyhow::Result<()> {
        const STOPS: usize = 8;
        let (client, server, _) = restricted_nats_peer(false).await?;
        let host = crate::host::HostBuilder::default().build()?.start().await?;
        let subscription = client.subscribe(host_subject(host.id())).await?;
        client.flush().await?;
        server.abort();
        let _ = server.await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while client.connection_state() != async_nats::connection::State::Disconnected {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        // Fill the client's bounded send channel so even the single publisher
        // cannot enqueue another response while the broker is unreachable.
        let mut saturated = false;
        for _ in 0..1024 {
            if tokio::time::timeout(
                Duration::from_millis(50),
                client.publish("test.backpressure", Vec::new().into()),
            )
            .await
            .is_err()
            {
                saturated = true;
                break;
            }
        }
        assert!(saturated, "NATS send channel did not fill");
        let mut pending = VecDeque::new();
        for index in 0..1024 {
            let mut msg = command_message(
                if index % 2 == 0 {
                    "workload.start"
                } else {
                    "workload.status"
                },
                br#"{"workloadId":"backpressure","workload":{}}"#.to_vec(),
            );
            msg.reply = Some("_INBOX.backpressure".into());
            pending.push_back(msg);
        }
        // Last, so they reach the loop once every slot and the reply queue are
        // taken by the commands above.
        for _ in 0..STOPS {
            let mut msg = command_message(
                "workload.stop",
                br#"{"workloadId":"backpressure"}"#.to_vec(),
            );
            msg.reply = Some("_INBOX.backpressure".into());
            pending.push_back(msg);
        }
        let handler = Arc::new(CountingReplies::default());
        let liveness = crate::host::probes::Liveness::new(Duration::from_millis(100));
        let (tx, rx) = oneshot::channel();
        let control = AttachedHostControl::new(
            tx,
            spawn_control_loop(
                host.clone(),
                client,
                subscription,
                pending,
                host.acquire_control()?,
                rx,
                HostControlLoopOptions {
                    handler: Some(handler.clone()),
                    liveness: Some(liveness.clone()),
                    heartbeat_interval: Duration::from_millis(10),
                    ..HostControlLoopOptions::default()
                },
            ),
        );
        // Every stop ran, though nothing had room for one: a stop turned away
        // is a workload left running. They were queued last, so this is also
        // the loop having worked through everything ahead of them.
        tokio::time::timeout(Duration::from_secs(5), async {
            while handler.stops.load(std::sync::atomic::Ordering::SeqCst) < STOPS {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .context("stops were shed while host control was saturated")?;
        let accepted = handler.accepted.load(std::sync::atomic::Ordering::SeqCst);
        assert!(accepted > 0);
        assert!(
            accepted <= MAX_PENDING_REPLIES + MAX_PENDING_STARTS + MAX_PENDING_QUERIES + 1,
            "unbounded commands ran while replies were stalled: {accepted}"
        );
        // Polled rather than read once: a stalled loop never beats again, while
        // a healthy one on a loaded machine is merely late.
        tokio::time::timeout(Duration::from_secs(5), async {
            while !liveness
                .silence()
                .is_some_and(|silence| silence < Duration::from_millis(100))
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("control loop stopped turning while replies were stalled")?;
        let error = tokio::time::timeout(Duration::from_secs(10), control.shutdown())
            .await?
            .err()
            .context("disconnected attached shutdown lost its error")?;
        assert!(error.to_string().contains("timed out"));
        assert_eq!(host.heartbeat().await?.id, host.id());
        host.stop().await
    }

    /// Port 1 is privileged and nothing in a test environment listens on it, so
    /// a connection there is refused rather than left hanging.
    const REFUSED: &str = "nats://127.0.0.1:1";

    /// A host and its NATS come up together with no ordering between them, so a
    /// refusal at startup is a race to wait out. Exiting instead spends a pod
    /// restart on it, and restarts are the signal that something went wrong.
    #[tokio::test]
    async fn a_refused_connection_is_retried_for_the_whole_window() {
        const WINDOW: Duration = Duration::from_millis(700);

        let started = std::time::Instant::now();
        let err = connect_nats(
            REFUSED,
            NatsConnectionOptions {
                connect_retry: Some(WINDOW),
                ..Default::default()
            },
        )
        .await
        .expect_err("nothing is listening on port 1");

        assert!(
            started.elapsed() >= WINDOW,
            "gave up after {:?}, before the {WINDOW:?} window was out",
            started.elapsed()
        );
        let message = format!("{err:#}");
        assert!(
            message.contains("within"),
            "the error should say the window it exhausted: {message}"
        );
    }

    /// Waiting is for the deployment that cannot order its own startup. A
    /// command run against a NATS the operator already has up wants the
    /// refusal, not a minute of patience.
    #[tokio::test]
    async fn without_a_window_a_refused_connection_is_reported_at_once() {
        let started = std::time::Instant::now();
        connect_nats(REFUSED, NatsConnectionOptions::default())
            .await
            .expect_err("nothing is listening on port 1");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a refusal with no window took {:?}, so it was retried",
            started.elapsed()
        );
    }

    /// A host always keeps a start it can run and a core it can serve on.
    #[test]
    fn starts_leave_a_core_to_serve_with() {
        assert_eq!(starts_for_cores(0), 1);
        assert_eq!(starts_for_cores(1), 1);
        assert_eq!(starts_for_cores(2), 1);
        assert_eq!(starts_for_cores(4), 3);
        assert_eq!(starts_for_cores(5), MAX_CONCURRENT_STARTS);
        assert_eq!(starts_for_cores(64), MAX_CONCURRENT_STARTS);
    }

    /// Every instance limit a component declares on the wire has to reach the
    /// runtime. An in-process test builds `types::Component` directly and so
    /// never crosses this conversion, which is where a limit that exists on
    /// both sides can go missing — leaving the knob unreachable from a
    /// workload deployed through the operator.
    #[test]
    fn wire_limits_reach_the_runtime() {
        let wire = types::v2::Component {
            name: "pooled".to_string(),
            pool_size: 4,
            max_invocations: 100,
            max_concurrency: 8,
            reclaim_window_seconds: 30,
            reclaim_min_instances: 2,
            ..Default::default()
        };

        let component = component_from_wire(
            &wire,
            LoadedComponent {
                bytes: b"\0asm".as_slice().into(),
                digest: Some("sha256:abc".to_string()),
            },
            crate::types::LocalResources::default(),
        );
        assert_eq!(component.name, "pooled");
        assert_eq!(component.digest.as_deref(), Some("sha256:abc"));
        assert_eq!(component.pool_size, 4);
        assert_eq!(component.max_invocations, 100);
        assert_eq!(component.max_concurrency, 8);
        assert_eq!(component.reclaim_window_seconds, 30);
        assert_eq!(component.reclaim_min_instances, 2);

        // And the runtime reads those limits as the policy they name.
        assert_eq!(
            crate::engine::InstancePolicy::from_component(&component),
            crate::engine::InstancePolicy::Warm {
                pool_size: std::num::NonZeroUsize::new(4).unwrap(),
                max_invocations: std::num::NonZeroUsize::new(100),
                max_concurrency: std::num::NonZeroUsize::new(8).unwrap(),
                reclaim: Some(crate::engine::ReclaimPolicy {
                    window: std::time::Duration::from_secs(30),
                    min_instances: 2,
                }),
            }
        );
    }

    #[test]
    fn try_from_v2_local_resources_parses_allowed_hosts() {
        // Strings flowing in from the proto wire are parsed into typed
        // `AllowedHost` entries. Valid forms should round-trip cleanly.
        let proto = types::v2::LocalResources {
            memory_limit_mb: 0,
            cpu_limit: 0,
            config: Default::default(),
            environment: Default::default(),
            volume_mounts: vec![],
            allowed_hosts: vec![
                "*".to_string(),
                "*.example.com".to_string(),
                "api.example.com:8443".to_string(),
                "https://api.example.com".to_string(),
            ],
            allowed_ip_name_lookups: vec!["*.example.com".to_string(), "127.0.0.1".to_string()],
            allowed_host_loopback_ports: vec![],
        };
        let lr = crate::types::LocalResources::try_from(proto).expect("conversion should succeed");
        assert_eq!(lr.allowed_ip_name_lookups.len(), 2);
        assert!(matches!(
            lr.allowed_ip_name_lookups[0],
            AllowedIpName::SuffixWildcard { .. }
        ));
        assert!(matches!(
            lr.allowed_ip_name_lookups[1],
            AllowedIpName::Ip(_)
        ));
        assert_eq!(lr.allowed_hosts.len(), 4);
        assert!(matches!(lr.allowed_hosts[0], AllowedHost::Any));
        assert!(matches!(
            lr.allowed_hosts[1],
            AllowedHost::SuffixWildcard { .. }
        ));
        assert!(matches!(lr.allowed_hosts[2], AllowedHost::Authority(_)));
        assert!(matches!(lr.allowed_hosts[3], AllowedHost::Url(_)));
    }

    #[test]
    fn try_from_v2_local_resources_rejects_bad_allowed_hosts_entry() {
        // An ambiguous wildcard (`*com` matches every .com) must be
        // rejected — the K8s pattern guards this at admission time, but
        // workloads constructed by other paths land here and the
        // workload start should surface a clear error rather than
        // silently widen egress.
        let proto = types::v2::LocalResources {
            memory_limit_mb: 0,
            cpu_limit: 0,
            config: Default::default(),
            environment: Default::default(),
            volume_mounts: vec![],
            allowed_hosts: vec!["*com".to_string()],
            allowed_ip_name_lookups: vec![],
            allowed_host_loopback_ports: vec![],
        };
        let err = crate::types::LocalResources::try_from(proto)
            .expect_err("conversion should reject ambiguous wildcard");
        let msg = format!("{err:#}");
        assert!(msg.contains("*com"), "{msg}");
        assert!(
            msg.contains("leading dot") || msg.contains("invalid"),
            "{msg}"
        );
    }

    #[test]
    fn try_from_v2_local_resources_collects_all_allowed_hosts_errors() {
        // Three bad entries in one workload — the conversion should report
        // all three in a single error so the user doesn't have to iterate
        // fix → start → fail → fix → start → ... for each entry.
        let proto = types::v2::LocalResources {
            memory_limit_mb: 0,
            cpu_limit: 0,
            config: Default::default(),
            environment: Default::default(),
            volume_mounts: vec![],
            allowed_hosts: vec![
                "*com".to_string(),                       // bare-star wildcard
                "https://api.example.com/v1".to_string(), // has path
                "example.com:notaport".to_string(),       // bad port
            ],
            allowed_ip_name_lookups: vec![],
            allowed_host_loopback_ports: vec![],
        };
        let err = crate::types::LocalResources::try_from(proto)
            .expect_err("conversion should reject all bad entries");
        let msg = format!("{err:#}");
        assert!(msg.contains("*com"), "{msg}");
        assert!(msg.contains("/v1"), "{msg}");
        assert!(msg.contains("notaport"), "{msg}");
    }

    #[tokio::test]
    async fn test_image_pull_secret_to_oci_config_none() {
        let host_config = HostConfig {
            allow_oci_insecure: false,
            ..Default::default()
        };
        let secret: Option<types::v2::ImagePullSecret> = None;
        let config = image_pull_secret_to_oci_config(&host_config, &secret);
        assert!(config.credentials.is_none());
        assert!(!config.insecure);
    }

    #[tokio::test]
    async fn test_image_pull_secret_to_oci_config_basic_auth() {
        let secret = Some(types::v2::ImagePullSecret {
            username: "testuser".to_string(),
            password: "testpass".to_string(),
        });

        let host_config = HostConfig {
            allow_oci_insecure: false,
            ..Default::default()
        };
        let config = image_pull_secret_to_oci_config(&host_config, &secret);
        assert_eq!(
            config.credentials,
            Some(("testuser".to_string(), "testpass".to_string()))
        );
    }
}
