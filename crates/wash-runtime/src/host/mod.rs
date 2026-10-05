//! Host runtime for managing WebAssembly workloads and plugins.
//!
//! The host module provides the runtime environment for executing WebAssembly
//! workloads. It manages the lifecycle of components, coordinates with plugins
//! to provide capabilities, and handles system resources.
//!
//! # Key Components
//!
//! - [`Host`] - The main runtime that manages workloads and plugins
//! - [`HostBuilder`] - Builder for configuring host settings
//! - [`HostApi`] - Trait defining the host's external API
//! - [`HostWorkload`] - Internal representation of workload states
//!
//! # Architecture
//!
//! The host acts as the central coordinator between:
//! - WebAssembly components that need execution
//! - Plugins that provide WASI and other capabilities
//! - System resources like networking and storage
//! - External consumers through the HostApi
//!
//! # Example
//!
//! ```no_run
//! use wash_runtime::host::{HostBuilder, HostApi};
//! use wash_runtime::engine::Engine;
//! use std::sync::Arc;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let engine = Engine::builder().build()?;
//! let host = HostBuilder::new()
//!     .with_engine(engine)
//!     .with_friendly_name("my-host")
//!     .build()?;
//!
//! let host = host.start().await?;
//! let heartbeat = host.heartbeat().await?;
//! println!("Host {} is running", heartbeat.friendly_name);
//! # Ok(())
//! # }
//! ```

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use names::{Generator, Name};
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, trace, warn};
use wasmtime::component::Component;

use crate::engine::workload::ResolvedWorkload;
use crate::engine::{Engine, uses_wasi_http};
use crate::observability::{FuelConsumptionMeter, Meters};
use crate::plugin::{HostPlugin, WorkloadFailure, WorkloadFailureSink};
use crate::types::*;
use crate::wit::{WitInterface, WitWorld};

pub(crate) mod accept;
pub mod allowed_loopback;
mod cleanup;
pub mod declared_port;
pub mod egress_policy;
pub mod ports;
pub mod probes;
pub mod quota;
pub(crate) mod sysinfo;
use sysinfo::SystemMonitor;

pub mod allowed_hosts;
pub mod allowed_ip_name;
pub mod client_identity;
pub mod http;
pub mod http_client;
pub mod http_p3;
#[cfg(feature = "host-component-plugins")]
pub(crate) mod job_registry;
pub mod trigger_service;

/// The API for interacting with a wasmcloud host.
///
/// This trait defines the core operations for managing workloads on a host,
/// including starting, stopping, and querying workload status, as well as
/// retrieving host health information.
pub trait HostApi {
    /// Request a heartbeat containing the host's current state and system information.
    ///
    /// # Returns
    /// A `HostHeartbeat` containing system metrics, version info, and capability information.
    ///
    /// # Errors
    /// Returns an error if system information cannot be retrieved.
    fn heartbeat(&self) -> impl Future<Output = anyhow::Result<HostHeartbeat>>;
    /// Start a new workload on this host.
    ///
    /// # Arguments
    /// * `request` - Contains the workload configuration to start
    ///
    /// # Returns
    /// A `WorkloadStartResponse` with the status of the started workload.
    ///
    /// # Errors
    /// Returns an error if the workload fails to start or validate.
    fn workload_start(
        &self,
        request: WorkloadStartRequest,
    ) -> impl Future<Output = anyhow::Result<WorkloadStartResponse>>;
    /// Query the status of a running workload.
    ///
    /// # Arguments
    /// * `request` - Contains the workload ID to query
    ///
    /// # Returns
    /// A `WorkloadStatusResponse` with the current state of the workload.
    ///
    /// # Errors
    /// Returns an error if the workload is not found.
    fn workload_status(
        &self,
        request: WorkloadStatusRequest,
    ) -> impl Future<Output = anyhow::Result<WorkloadStatusResponse>>;
    /// Stop a running workload on this host.
    ///
    /// # Arguments
    /// * `request` - Contains the workload ID to stop
    ///
    /// # Returns
    /// A `WorkloadStopResponse` with the final status of the stopped workload.
    ///
    /// # Errors
    /// Returns an error if the workload cannot be stopped or is not found.
    fn workload_stop(
        &self,
        request: WorkloadStopRequest,
    ) -> impl Future<Output = anyhow::Result<WorkloadStopResponse>>;
}

// Helper trait impl that helps with Arc-ing the Host
impl<T: HostApi> HostApi for Arc<T> {
    async fn heartbeat(&self) -> anyhow::Result<HostHeartbeat> {
        self.as_ref().heartbeat().await
    }
    async fn workload_start(
        &self,
        request: WorkloadStartRequest,
    ) -> anyhow::Result<WorkloadStartResponse> {
        self.as_ref().workload_start(request).await
    }
    async fn workload_stop(
        &self,
        request: WorkloadStopRequest,
    ) -> anyhow::Result<WorkloadStopResponse> {
        self.as_ref().workload_stop(request).await
    }
    async fn workload_status(
        &self,
        request: WorkloadStatusRequest,
    ) -> anyhow::Result<WorkloadStatusResponse> {
        self.as_ref().workload_status(request).await
    }
}

/// The claim-then-start protocol a caller needs when it has work to do between
/// deciding to start a workload and having something the host can run.
///
/// Kept off [`HostApi`] deliberately. Its three calls only make sense together,
/// every reservation has to reach [`WorkloadReservation::workload_start_reserved`]
/// or [`WorkloadReservation::workload_release`], or the id sits in `Starting`
/// with nothing on the way to fill it — and that is bookkeeping to keep inside
/// this crate rather than an obligation to hand to everyone who implements the
/// host's API.
pub(crate) trait WorkloadReservation {
    /// Claim a workload id before doing the work a start needs.
    ///
    /// A caller that has to fetch images first would otherwise leave the id in
    /// no map for as long as that takes: `workload_status` reports it missing
    /// and `workload_stop` reports it already gone, so a stop arriving in that
    /// window tells its caller the teardown is done while the start goes on to
    /// run the workload. Reserving first puts the id in `Starting` for the
    /// whole window, where both of those read it correctly and a stop hands the
    /// teardown back to the start.
    ///
    /// `Err` carries the refusal to report, and reserves nothing.
    fn workload_reserve(
        &self,
        workload_id: &str,
    ) -> impl Future<Output = Result<Reservation, String>>;

    /// Give back an id claimed by [`WorkloadReservation::workload_reserve`]
    /// whose start never began. Does nothing if the id has moved on to another
    /// owner.
    #[cfg_attr(not(feature = "washlet"), allow(dead_code))]
    fn workload_release(
        &self,
        workload_id: &str,
        reservation: Reservation,
    ) -> impl Future<Output = ()>;

    /// Start a workload under an id already claimed by
    /// [`WorkloadReservation::workload_reserve`].
    fn workload_start_reserved(
        &self,
        reservation: Reservation,
        request: WorkloadStartRequest,
    ) -> impl Future<Output = anyhow::Result<WorkloadStartResponse>>;
}

/// A claim on one workload id, minted whenever a task takes responsibility for
/// that id and never reused within a host.
///
/// Both states a task has to leave behind while it works outside the map's lock
/// carry one, so the task can tell its own slot from a slot a *later* workload
/// claimed under the same id: a start reserves the id before building anything,
/// and a teardown marks the id while it releases. Every write back into the map
/// is conditional on the slot still holding the writer's own reservation.
pub type Reservation = u64;

/// Internal representation of a workload's state within the host.
///
/// This enum tracks the lifecycle stages of a workload from starting
/// through running to stopping or error states.
#[derive(Debug, Clone)]
pub enum HostWorkload {
    /// A start holds the id and is building the workload. The [`Reservation`]
    /// is that start's.
    Starting(Reservation),
    // Boxed to reduce size of the enum
    Running(Box<ResolvedWorkload>),
    /// The workload is being torn down, and the id stays reserved until whoever
    /// owns that teardown finishes it. The [`Reservation`] names the owner: the
    /// stop or failure that took the workload out of the map, or — when a stop
    /// arrived while the workload was still starting — the start that has yet
    /// to hand its work over.
    Stopping(Reservation),
    /// A plugin failed the workload while it was still starting. The start
    /// that holds the [`Reservation`] releases what it built and then leaves
    /// the reason as the workload's `Error`; until then the id stays reserved.
    Failing(Reservation, String),
    Error(String),
}

/// Give back everything binding a workload allocated: its service, then its
/// plugins.
///
/// Plugin failures release committed workloads here. Starts and stops use
/// their [`crate::engine::workload::WorkloadResources`] journal so cancellation
/// can resume cleanup without repeating completed steps.
///
/// Which slot in the workload map may be written, and by whom, is what keeps
/// this safe to run outside the map's lock: `unbind_all_plugins` is keyed by
/// workload id, so the id must stay reserved until this returns or a new
/// workload could claim it and be torn down by someone else's teardown. Every
/// caller therefore leaves a [`HostWorkload::Stopping`] carrying its own
/// [`Reservation`] over the whole call. See [`HostApi::workload_stop`] for the
/// ownership rules that guarantee it.
async fn release(workload_id: &str, resolved: &ResolvedWorkload) {
    resolved.begin_teardown();
    if let Err(e) = resolved.unbind_all_plugins().await {
        warn!(
            workload_id,
            error = ?e,
            "error unbinding plugins during teardown, continuing"
        );
    }
}

/// What a stop does to the slot it found, decided before anything is written.
enum StopAction {
    /// Mark the id `Stopping` under this reservation, holding it for the
    /// teardown that follows.
    Mark(Reservation),
    /// Leave the slot to whoever already owns it.
    Leave,
    /// Drop the id: there is nothing bound behind it.
    Drop,
}

impl std::fmt::Display for HostWorkload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostWorkload::Starting(_) => write!(f, "Starting"),
            HostWorkload::Running(_) => write!(f, "Running"),
            HostWorkload::Stopping(_) => write!(f, "Stopping"),
            HostWorkload::Failing(_, err) => write!(f, "Failing: {err}"),
            HostWorkload::Error(err) => write!(f, "Error: {err}"),
        }
    }
}

impl From<&HostWorkload> for WorkloadState {
    fn from(hw: &HostWorkload) -> Self {
        match hw {
            HostWorkload::Starting(_) => WorkloadState::Starting,
            HostWorkload::Running(_) => WorkloadState::Running,
            HostWorkload::Stopping(_) | HostWorkload::Failing(..) => WorkloadState::Stopping,
            HostWorkload::Error(_) => WorkloadState::Error,
        }
    }
}

/// A wasmcloud host that manages WebAssembly workloads and plugins.
///
/// The `Host` is the primary runtime for executing workloads. It manages:
/// - An engine for compiling and running WebAssembly components
/// - A collection of workloads and their states
/// - Plugins that extend host functionality
/// - System monitoring and resource tracking
pub struct Host {
    engine: Engine,
    /// Workloads mapped from ID to the workload and its current state
    workloads: Arc<RwLock<HashMap<String, HostWorkload>>>,
    /// Interrupted workload operations retained until cleanup succeeds, allowing
    /// workload stops to retry failed teardown without reusing their IDs.
    workload_recoveries: cleanup::WorkloadRecoveries,
    /// Background cleanup spawned when workload starts or stops are cancelled;
    /// awaited before control or host shutdown completes.
    workload_cleanup_tasks: tokio_util::task::TaskTracker,
    /// Source of the [`Reservation`] a start or a teardown stamps on the slot it
    /// owns. Monotonic for the life of the host, so a reservation identifies one
    /// occupant of a workload id and never a later one.
    reservations: std::sync::atomic::AtomicU64,
    /// Set by [`Self::stop`], and read by [`Self::workload_reserve`] under the
    /// `workloads` write lock that both take — so a start either reserves
    /// before the stop or is refused by it, never lands between the two. A
    /// stopped host is on its way out: its ingress has stopped accepting and
    /// its plugins are stopping, so a workload started onto one would be
    /// unroutable and, once the ingress drain ends, silently unregistered.
    stopped: std::sync::atomic::AtomicBool,
    /// Attachment lease checked when attaching control to prevent a second
    /// loop while commands or cleanup still hold it. Weak lets those owners
    /// release the lease without the host keeping it alive.
    #[cfg(feature = "washlet")]
    control: std::sync::Mutex<std::sync::Weak<HostControlLease>>,
    /// Plugins in a map from their ID to the plugin itself
    plugins: HashMap<&'static str, Arc<dyn HostPlugin>>,
    /// What the operator declared about each plugin's bindings — the host layer
    /// under every workload's own `interface-binding` config, and who is
    /// allowed to write it.
    plugin_bindings: Arc<crate::plugin::PluginBindings>,
    /// Host metadata
    id: String,
    hostname: String,
    friendly_name: String,
    environment: String,
    version: String,
    labels: HashMap<String, String>,
    started_at: chrono::DateTime<chrono::Utc>,
    /// System monitor for tracking CPU/memory usage
    system_monitor: Arc<RwLock<SystemMonitor>>,
    // endpoints: HashMap<String, EndpointConfiguration>
    pub(crate) http_handler: std::sync::Arc<dyn crate::host::http::HostHandler>,
    /// Keeps the HTTP ingress marked as host-owned for this host's lifetime.
    _port_reservations: Vec<crate::host::ports::PortReservation>,
    /// This host's own lifetime, handed out weakly; see [`HostLifetime`].
    lifetime: Arc<HostLifetime>,
    config: HostConfig,
    meters: Meters,
}

/// A token whose lifetime is a [`Host`]'s.
///
/// Only the builder and host hold this token strongly. Callers get weak refs.
struct HostLifetime(());

/// Shared by the control loop and its commands, so cancellation cannot release
/// control ownership while a command still holds the host.
#[cfg(feature = "washlet")]
pub(crate) struct HostControlLease;

/// A weak link to a host's lifetime and optional HTTP handler. Every guest
/// store uses the lifetime check, even without HTTP egress.
///
/// Weak throughout, and for the handler deliberately so — see
/// [`crate::host::http::live_handler`].
#[derive(Clone)]
pub struct HostRef {
    handler: Option<std::sync::Weak<dyn crate::host::http::HostHandler>>,
    /// `None` for a reference built from a handler alone, by an embedder
    /// wiring stores up without a [`Host`]: there is no host whose teardown
    /// could be detected, which [`Self::host_is_gone`] answers honestly rather
    /// than guessing from the handler.
    lifetime: Option<std::sync::Weak<HostLifetime>>,
}

impl HostRef {
    /// A reference to a handler with no host behind it, for an embedder that
    /// builds stores itself. Egress works; nothing ends those stores when the
    /// handler goes, because no host owns them.
    pub fn from_handler(handler: &Arc<dyn crate::host::http::HostHandler>) -> Self {
        Self {
            handler: Some(Arc::downgrade(handler)),
            lifetime: None,
        }
    }

    /// The configured HTTP handler, while it remains available.
    pub fn handler(&self) -> Option<Arc<dyn crate::host::http::HostHandler>> {
        if self.host_is_gone() {
            return None;
        }
        self.handler.as_ref().and_then(std::sync::Weak::upgrade)
    }

    pub(crate) fn has_handler(&self) -> bool {
        self.handler.is_some()
    }

    /// Whether the host this refers to has been torn down.
    pub(crate) fn host_is_gone(&self) -> bool {
        self.lifetime
            .as_ref()
            .is_some_and(|lifetime| lifetime.strong_count() == 0)
    }
}

impl Host {
    pub(crate) fn workload_cleanup_guard(
        &self,
        workload_id: &str,
        reservation: Reservation,
    ) -> cleanup::WorkloadCleanupGuard {
        cleanup::WorkloadCleanupGuard::new(self, workload_id, reservation)
    }

    /// Wait for every interrupted start or stop to finish its teardown.
    ///
    /// A teardown that keeps failing is retried in the background for as long
    /// as the host runs, so this returns only once all of them have succeeded;
    /// callers bound it. The error names the workloads still owed a teardown,
    /// for the one case a recovery is recorded with nothing retrying it: an
    /// operation dropped outside the runtime, which a workload stop cleans up.
    pub(crate) async fn wait_for_workload_cleanup(&self) -> anyhow::Result<()> {
        self.workload_cleanup_tasks.wait().await;
        let pending = cleanup::pending_recoveries(&self.workload_recoveries);
        anyhow::ensure!(
            pending.is_empty(),
            "interrupted workloads {pending:?} still hold their resources; a workload stop \
             retries their cleanup"
        );
        Ok(())
    }

    #[cfg(feature = "washlet")]
    pub(crate) fn acquire_control(&self) -> anyhow::Result<Arc<HostControlLease>> {
        anyhow::ensure!(
            !self.stopped.load(std::sync::atomic::Ordering::SeqCst),
            "cannot attach control to a stopped host"
        );
        let mut control = self
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The lease outlives its attachment for as long as a command or an
        // operation begun under it is still running or being cleaned up, so say
        // that too: the caller may have shut the last attachment down already.
        // Name the workloads whose cleanup is still owed, because the caller
        // can hurry one along with a workload stop only if it knows which.
        if control.upgrade().is_some() {
            let pending = cleanup::pending_recoveries(&self.workload_recoveries);
            anyhow::bail!(
                "host already has an active control attachment, or a workload operation begun \
                 under the previous one is still running or being cleaned up (workloads still \
                 being cleaned up: {pending:?})"
            );
        }
        let lease = Arc::new(HostControlLease);
        *control = Arc::downgrade(&lease);
        Ok(lease)
    }

    /// Create a new builder for the host.
    pub fn builder() -> HostBuilder {
        HostBuilder::default()
    }

    /// What this host hands to everything it builds so it can reach back; see
    /// [`HostRef`].
    pub fn host_ref(&self) -> HostRef {
        HostRef {
            handler: Some(Arc::downgrade(&self.http_handler)),
            lifetime: Some(Arc::downgrade(&self.lifetime)),
        }
    }

    /// Extract known WIT interfaces from a component's imports and exports
    ///
    /// Inspects the component to determine what interfaces it uses and provides.
    /// This is used to populate the `host_interfaces` field in the Workload, which is
    /// checked bidirectionally against both imports and exports during plugin binding.
    ///
    /// For example:
    /// - A component that **imports** `wasi:blobstore/blobstore` needs the blobstore plugin
    pub fn intersect_interfaces(
        &self,
        component_bytes: &[u8],
    ) -> anyhow::Result<HashSet<WitInterface>> {
        // Create a minimal engine just for introspection
        let engine = self.engine.inner();
        let component = Component::new(engine, component_bytes)
            .map_err(anyhow::Error::from)
            .context("failed to parse component for interface extraction")?;
        let ty = component.component_type();

        let mut interfaces = HashSet::new();

        let parse_interface = |name: &str| -> Option<WitInterface> {
            // Parse names like "wasi:http/incoming-handler@0.2.0"
            let (namespace_package, interface_version) = name.rsplit_once('/')?;
            let (namespace, package) = namespace_package.split_once(':')?;

            // Extract interface name and optional version
            let (interface, version) = if let Some((iface, ver)) = interface_version.split_once('@')
            {
                let parsed_version = ver.parse().ok();
                (iface.to_string(), parsed_version)
            } else {
                (interface_version.to_string(), None)
            };

            Some(WitInterface {
                namespace: namespace.to_string(),
                package: package.to_string(),
                interfaces: HashSet::from([interface]),
                version,
                config: HashMap::new(),
                name: None,
            })
        };

        let mut filter_plugins = |interface: &WitInterface| {
            let mut found = false;
            for (_, plugin) in self.plugins.iter() {
                if plugin.world().includes(interface) {
                    found = true;
                    break;
                }
            }
            if found {
                interfaces.insert(interface.clone());
            }
        };

        // Extract imports (filter out standard WASI interfaces)
        for (import_name, _item) in ty.imports(engine) {
            if let Some(interface) = parse_interface(import_name) {
                filter_plugins(&interface);
            }
        }

        // Extract exports (these are what the component provides to plugins)
        for (export_name, _item) in ty.exports(engine) {
            if let Some(interface) = parse_interface(export_name) {
                filter_plugins(&interface);
            }
        }

        // http is not a plugin
        if uses_wasi_http(&component) {
            interfaces.insert(WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: HashSet::from([
                    "incoming-handler".to_string(),
                    "outgoing-handler".to_string(),
                ]),
                version: None,
                config: HashMap::new(),
                name: None,
            });
        }

        Ok(interfaces)
    }

    /// Start the host and initialize all plugins.
    ///
    /// This method must be called before the host can accept workloads.
    /// It starts all registered plugins and prepares the host for operation.
    ///
    /// # Returns
    /// An `Arc` wrapped host ready to accept workloads.
    ///
    /// # Errors
    /// Returns an error if any plugin fails to start.
    pub async fn start(self) -> anyhow::Result<Arc<Self>> {
        self.http_handler.inject_meters(&self.meters).await;

        self.http_handler
            .start()
            .await
            .context("failed to start HTTP handler")?;

        // A plugin can fail a workload out of band (a host component plugin
        // evicting one whose lifecycle bind crash-loops). Give each plugin a
        // sink to report that on, drained by a background task that transitions
        // the workload to a failed state.
        let (failure_tx, failure_rx) = tokio::sync::mpsc::unbounded_channel();
        let failure_sink = WorkloadFailureSink::new(failure_tx);

        // Start all plugins, any errors means the host fails to start. The
        // failure sink is injected before `start` so a plugin that evicts a
        // workload immediately still has somewhere to report it.
        for (id, plugin) in &self.plugins {
            plugin.inject_meters(&self.meters).await;
            plugin.set_workload_failure_sink(failure_sink.clone());

            if let Err(e) = plugin.start().await {
                tracing::error!(id = id, err = ?e, "failed to start plugin");
                bail!(e)
            }
        }

        let host = Arc::new(self);
        // Weak, not strong: the sinks handed to the plugins live inside
        // `host.plugins`, so the channel stays open for as long as the host
        // does. A strong handle here would therefore be a cycle — the drain
        // would keep the host alive, and the host would keep the drain's
        // channel open — leaking the host, its engine, and every compiled
        // component. `Host::stop` cannot break it either: it stops the plugins
        // but never drops them.
        tokio::spawn(consume_workload_failures(Arc::downgrade(&host), failure_rx));
        Ok(host)
    }

    /// Mint a fresh [`Reservation`] for a slot this host is about to claim.
    fn reserve(&self) -> Reservation {
        self.reservations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Close out a teardown that marked `workload_id` as
    /// `Stopping(reservation)` or `Failing(reservation, ..)`: drop the id, or
    /// leave the failure behind as the workload's `Error` for a later stop to
    /// collect.
    ///
    /// Conditional on the marker still being this teardown's, because a
    /// teardown runs outside the map's lock. If the slot holds anything else —
    /// most of all a *different* workload that claimed the id in the meantime —
    /// it is not this teardown's to write, and removing it would leave that
    /// workload running with nothing tracking it.
    async fn finish_teardown(
        &self,
        workload_id: &str,
        reservation: Reservation,
        reason: Option<String>,
    ) {
        let mut workloads = self.workloads.write().await;
        let failure = match workloads.get(workload_id) {
            Some(HostWorkload::Stopping(held)) if *held == reservation => None,
            Some(HostWorkload::Failing(held, failure)) if *held == reservation => {
                Some(failure.clone())
            }
            _ => return,
        };
        match reason.or(failure) {
            Some(reason) => {
                workloads.insert(workload_id.to_string(), HostWorkload::Error(reason));
            }
            None => {
                workloads.remove(workload_id);
            }
        }
    }

    /// Transition a running workload to a failed state on a plugin's report
    /// (e.g. an evicted crash-looping bind): swap it to `Error`, so its status
    /// reports failed, and tear down its resources like a stop would. A workload
    /// that is already gone or not running is left as-is.
    async fn fail_workload(&self, workload_id: &str, reason: String) {
        // Mark `Stopping` under a reservation of this failure's own rather than
        // writing `Error` straight away. `Error` is a state a stop frees the id
        // from, and freeing it here would let a redeploy claim the same id while
        // `release` — keyed by workload id — is still unbinding, tearing down
        // the new workload's bindings instead. The reservation is what makes the
        // marker this failure's: nobody else writes over it, and the `Error`
        // below lands only if it is still there.
        let reservation = self.reserve();
        let resolved = {
            let mut workloads = self.workloads.write().await;
            match workloads.get_mut(workload_id) {
                Some(slot @ HostWorkload::Running(_)) => {
                    match std::mem::replace(slot, HostWorkload::Stopping(reservation)) {
                        HostWorkload::Running(rw) => Some(*rw),
                        // Just matched on it.
                        _ => None,
                    }
                }
                // Still starting: no teardown follows here, because the start
                // owns everything it has built. `Failing` under the start's own
                // reservation tells it its slot is gone while keeping the id
                // held, so it releases what it bound and then publishes the
                // reason as `Error`.
                Some(slot @ HostWorkload::Starting(_)) => {
                    if let HostWorkload::Starting(held) = *slot {
                        *slot = HostWorkload::Failing(held, reason.clone());
                    }
                    None
                }
                // Already being torn down, already failed, or gone: the workload
                // is on its way out either way, and the slot belongs to whoever
                // is finishing it.
                Some(
                    HostWorkload::Stopping(_) | HostWorkload::Failing(..) | HostWorkload::Error(_),
                )
                | None => None,
            }
        };
        if let Some(resolved) = resolved {
            release(workload_id, &resolved).await;
            // The id was held as `Stopping` for the teardown; publish the
            // failure now that letting a stop free it is safe.
            self.finish_teardown(workload_id, reservation, Some(reason.clone()))
                .await;
        }
        warn!(
            workload_id,
            reason, "workload failed by a plugin; marked as errored"
        );
    }

    /// Stop the host and shut down all plugins.
    ///
    /// Attempts to gracefully stop all plugins, allowing each the
    /// `WASH_PLUGIN_STOP_TIMEOUT_SECS` budget plus a one-second grace.
    /// Errors are logged but don't prevent other plugins from being
    /// stopped.
    ///
    /// # Returns
    /// Ok if the shutdown process completes (even with plugin errors).
    pub async fn stop(self: Arc<Self>) -> anyhow::Result<()> {
        // Before anything else stops, and under the lock `workload_reserve`
        // takes, so no start can be admitted from here on.
        {
            let _workloads = self.workloads.write().await;
            self.stopped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }

        self.http_handler
            .stop()
            .await
            .context("failed to stop HTTP handler")?;

        // Interrupted starts and stops finish teardown before global plugin
        // shutdown. Bounded, because a teardown a plugin keeps refusing is
        // retried for as long as the host runs: past the budget the plugins
        // are stopped anyway, and clearing the recoveries below ends the
        // retries.
        let cleanup_timeout = crate::timeouts::plugin_stop() + std::time::Duration::from_secs(1);
        match tokio::time::timeout(cleanup_timeout, self.wait_for_workload_cleanup()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "stopping plugins with failed workload cleanup");
            }
            Err(_) => {
                let pending = cleanup::pending_recoveries(&self.workload_recoveries);
                tracing::warn!(
                    workloads = ?pending,
                    timeout_secs = cleanup_timeout.as_secs(),
                    "stopping plugins with interrupted workload cleanup still running"
                );
            }
        }

        // Stop all plugins, log errors but continue stopping others. The cap
        // must outlast the plugin-stop budget: a host component plugin's
        // `stop()` waits the full budget for its supervisor and only then
        // aborts it, and if the outer timeout fired first it would drop that
        // future — and the supervisor's JoinHandle with it — detaching a
        // wedged task instead of aborting it. The one-second grace covers the
        // abort-and-return tail past the inner wait; that tail must stay
        // synchronous (or bounded well under the grace) for the guarantee to
        // hold, so keep awaits out of the post-timeout path in
        // `ComponentHostPlugin::stop`.
        let stop_timeout = crate::timeouts::plugin_stop() + std::time::Duration::from_secs(1);
        for (id, plugin) in &self.plugins {
            let stop_fut = plugin.stop();
            match tokio::time::timeout(stop_timeout, stop_fut).await {
                Ok(Err(e)) => {
                    tracing::error!(id = id, err = ?e, "failed to stop plugin");
                }
                Err(_) => {
                    tracing::error!(
                        id = id,
                        timeout_secs = stop_timeout.as_secs(),
                        "plugin stop timed out"
                    );
                }
                _ => {}
            }
        }

        self.workload_recoveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        Ok(())
    }

    /// Get a label value by key.
    ///
    /// # Arguments
    /// * `label` - The label key to look up
    ///
    /// # Returns
    /// The label value if it exists, None otherwise.
    pub fn label(&self, label: impl AsRef<str>) -> Option<&String> {
        self.labels.get(label.as_ref())
    }

    /// Get the unique identifier for this host.
    ///
    /// # Returns
    /// The host's unique ID string.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the system hostname for this host.
    ///
    /// # Returns
    /// The host's system hostname string.
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    /// Get all labels assigned to this host.
    ///
    /// # Returns
    /// A reference to the host's labels map.
    pub fn labels(&self) -> &HashMap<String, String> {
        &self.labels
    }

    /// Get the version of this host.
    ///
    /// # Returns
    /// The host's version string.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Get host config
    ///
    /// # Returns
    /// The host's config
    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Get the human-readable name for this host.
    ///
    /// # Returns
    /// The host's friendly name string.
    pub fn friendly_name(&self) -> &str {
        &self.friendly_name
    }

    /// Get the environment this host advertises itself as running in.
    ///
    /// For Kubernetes host pods this is the pod's namespace; for
    /// out-of-cluster hosts it is whatever was passed via
    /// [`HostBuilder::with_environment`]. Empty when no environment was
    /// configured.
    pub fn environment(&self) -> &str {
        &self.environment
    }

    /// Returns the WIT (imports, exports) that this host can provide to any component.
    ///
    /// Put another way, this represents a simplified version of the host world. For
    /// example, this WIT world:
    /// ```wit
    /// package wasmcloud:host@0.1.0;
    ///
    /// interface foo {
    /// ...
    /// }
    /// interface bar {
    /// ...
    /// }
    ///
    /// world host {
    ///   import foo;
    ///   export bar;
    /// }
    /// ```
    ///
    /// Would be returned as:
    /// (
    ///  vec![WitInterface { namespace: "wasmcloud", package: "host", interfaces: ["foo"], version: Some("0.1.0") }],
    ///  vec![WitInterface { namespace: "wasmcloud", package: "host", interfaces: ["bar"], version: Some("0.1.0") }],
    /// )
    ///
    /// This can be viewed as an inversion of the worlds that this host can support. In the above example,
    /// this host can support any component that imports `bar` and exports `foo`. Other exports will be ignored,
    /// and other imports that are unsatisfied will be rejected.
    pub fn wit_world(&self) -> WitWorld {
        let mut imports = HashSet::new();
        // The host provides wasi@0.2 interfaces other than wasi:http
        // <https://docs.rs/wasmtime-wasi/36.0.2/wasmtime_wasi/p2/index.html#wasip2-interfaces>
        let mut exports = HashSet::from([
            "wasi:http/types,incoming-handler,outgoing-handler@0.2.0".into(),
            "wasi:io/poll,error,streams@0.2.0".into(),
            "wasi:clocks/monotonic-clock,wall-time@0.2.0".into(),
            "wasi:random/random@0.2.0".into(),
            "wasi:cli/environment,exit,stderr,stdin,stdout,terminal-input,terminal-output,terminal-stderr,terminal-stdin,terminal-stdout@0.2.0".into(),
            "wasi:clocks/monotonic-clock,wall-clock@0.2.0".into(),
            "wasi:filesystem/preopens,types@0.2.0".into(),
            "wasi:random/insecure-seed,insecure,random@0.2.0".into(),
            "wasi:sockets/instance-network,ip-name-lookup,network,tcp-create-socket,tcp,udp-create-socket,udp@0.2.0".into(),
            "wasi:http/types,handler@0.3.0".into(),
            #[cfg(feature = "wasi-tls")]
            "wasi:tls/client,types@0.3.0-draft".into(),
        ]);

        // Include imports and exports that plugins specify
        imports.extend(
            self.plugins
                .values()
                .flat_map(|p| p.world().imports.into_iter().collect::<Vec<_>>()),
        );
        exports.extend(
            self.plugins
                .values()
                .flat_map(|p| p.world().exports.into_iter().collect::<Vec<_>>()),
        );

        WitWorld { imports, exports }
    }

    /// Logs all available host interfaces to the tracing system.
    pub fn log_interfaces(&self) {
        let wit_world = self.wit_world();

        // Collect and sort exports for consistent output
        let mut exports: Vec<_> = wit_world.exports.iter().collect();
        exports.sort_by(|a, b| (&a.namespace, &a.package).cmp(&(&b.namespace, &b.package)));

        let interfaces: Vec<String> = exports.iter().map(|e| e.to_string()).collect();
        info!(
            count = interfaces.len(),
            interfaces = ?interfaces,
            "Host provides interfaces"
        );
    }

    /// Returns a three-tuple of (OS architecture, OS name, OS kernel)
    async fn get_system_info(&self) -> (String, String, String) {
        // Get OS information
        let os_name = std::env::consts::OS.to_string();
        let os_arch = std::env::consts::ARCH.to_string();
        let os_kernel = std::env::consts::FAMILY.to_string();
        (os_arch, os_name, os_kernel)
    }

    /// Returns a tuple of (total memory, free memory)
    async fn get_memory_info(&self) -> anyhow::Result<(u64, u64)> {
        let monitor = self.system_monitor.read().await;
        let mem = monitor.memory_usage();
        Ok((mem.total_memory, mem.free_memory))
    }

    /// Returns the current global CPU usage as a percentage
    async fn get_cpu_usage(&self) -> anyhow::Result<f32> {
        let monitor = self.system_monitor.read().await;
        Ok(monitor.cpu_usage().global_usage)
    }

    async fn workload_start_inner(
        &self,
        request: WorkloadStartRequest,
        cleanup: &crate::engine::workload::WorkloadResources,
    ) -> anyhow::Result<ResolvedWorkload> {
        let service_present = request.workload.service.is_some();
        let workload_id = request.workload_id.clone();

        // Initialize the workload using the engine, receiving the unresolved workload.
        //
        // Cranelift compiles every component here, guarded by the moka cache.
        // This occupies whichever thread runs it for as long as compilation takes.
        // On a runtime thread, that may stall async-nats connection task alongside it
        // and a host that stops draining its socket is disconnected as a slow consumer.
        let engine = self.engine.clone();
        let init_id = request.workload_id;
        let workload = request.workload;
        let span = tracing::Span::current();
        let unresolved_workload = tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            engine.initialize_workload(&init_id, workload)
        })
        .await
        .context("workload initialization task failed")??;

        // Native rollback uses the start's journal, including partially bound
        // plugins, so a cancellation can resume the same teardown.
        let mut resolved_workload = unresolved_workload
            .resolve_for_start(
                Some(&self.plugins),
                &self.plugin_bindings,
                &self.host_ref(),
                &self.meters,
                Some(cleanup),
            )
            .await?;

        // The caller commits successful starts or releases their journal on failure.
        start_resolved(&workload_id, &mut resolved_workload, service_present).await?;

        cleanup.resolved(&resolved_workload);
        Ok(resolved_workload)
    }
}

/// Everything after plugin binding that can still fail while starting a
/// workload. Kept together so [`Host::workload_start_inner`] has exactly one
/// failure result to hand back to the start guard for rollback.
async fn start_resolved(
    workload_id: &str,
    resolved: &mut ResolvedWorkload,
    service_present: bool,
) -> anyhow::Result<()> {
    // If the service didn't run and we had one, warn
    if service_present && resolved.execute_service().await?.is_none() {
        warn!(workload_id, "service did not properly execute");
    }
    Ok(())
}

impl HostApi for Host {
    async fn heartbeat(&self) -> anyhow::Result<HostHeartbeat> {
        // Refresh system info before reporting
        {
            let mut monitor = self.system_monitor.write().await;
            monitor.refresh();
            monitor.report_usage();
        }
        // Reported beside the machine's own usage, because the two answer
        // different questions: the monitor says how much memory this process
        // holds, the budget says how much of it guests asked for.
        self.engine.guest_memory().report();

        let (os_arch, os_name, os_kernel) = self.get_system_info().await;
        let (system_memory_total, system_memory_free) = self
            .get_memory_info()
            .await
            .context("failed to get memory info")?;
        let system_cpu_usage = self
            .get_cpu_usage()
            .await
            .context("failed to get CPU usage")?;

        // Count components and providers from workloads
        let (workload_count, component_count) = {
            let workloads = self.workloads.read().await;
            let workload_count: u64 = workloads.len() as u64;
            let mut component_count: u64 = 0;
            for workload in workloads.values() {
                if let HostWorkload::Running(workload) = workload {
                    component_count += workload.component_count().await as u64;
                }
            }
            (workload_count, component_count)
        };

        // Collect all imports and exports from the host and plugins
        let mut imports = Vec::new();
        let mut exports = Vec::new();

        for plugin in self.plugins.values() {
            let world = plugin.world();
            imports.extend(world.imports);
            exports.extend(world.exports);
        }

        Ok(HostHeartbeat {
            id: self.id.clone(),
            hostname: self.hostname.clone(),
            friendly_name: self.friendly_name.clone(),
            environment: self.environment.clone(),
            http_port: self.http_handler.port(),
            version: self.version.clone(),
            labels: self.labels.clone(),
            started_at: self.started_at,
            os_arch,
            os_name,
            os_kernel,
            system_cpu_usage,
            system_memory_total,
            system_memory_free,
            component_count,
            workload_count,
            imports,
            exports,
        })
    }

    /// Start a workload
    #[instrument(skip_all, fields(workload.id = request.workload_id, workload.name = request.workload.name, workload.namespace = request.workload.namespace))]
    async fn workload_start(
        &self,
        request: WorkloadStartRequest,
    ) -> anyhow::Result<WorkloadStartResponse> {
        let workload_id = request.workload_id.clone();
        match self.workload_reserve(&workload_id).await {
            Ok(reservation) => self.workload_start_reserved(reservation, request).await,
            Err(message) => Ok(WorkloadStartResponse {
                workload_status: WorkloadStatus {
                    workload_id,
                    workload_state: WorkloadState::Error,
                    message,
                },
            }),
        }
    }

    #[instrument(skip_all, fields(workload.id = request.workload_id))]
    async fn workload_status(
        &self,
        request: WorkloadStatusRequest,
    ) -> anyhow::Result<WorkloadStatusResponse> {
        if let Some(workload) = self.workloads.read().await.get(&request.workload_id) {
            let workload_state = workload.into();
            // A failed workload reports the reason it failed, verbatim and
            // unprefixed. This is the only field that crosses to the operator,
            // and the operator puts it in the `Sync` condition — so it is the
            // one place a `kubectl`-only operator can learn *why* a workload
            // is in `WORKLOAD_STATE_ERROR`. Wrapping it in "Workload is
            // Error: " spends the front of a truncated condition message on
            // saying again what the state field already says.
            // Every other state is already named by `workload_state`, and the
            // operator formats both — so a message here renders the state
            // twice ("WORKLOAD_STATE_STARTING: Workload is Starting").
            let message = match workload {
                HostWorkload::Error(reason) => reason.clone(),
                _ => String::new(),
            };
            Ok(WorkloadStatusResponse {
                workload_status: WorkloadStatus {
                    workload_id: request.workload_id,
                    message,
                    workload_state,
                },
            })
        } else {
            let message = format!("Workload not found: {}", request.workload_id);
            Ok(WorkloadStatusResponse {
                workload_status: WorkloadStatus {
                    workload_id: request.workload_id,
                    message,
                    workload_state: WorkloadState::NotFound,
                },
            })
        }
    }

    #[instrument(skip_all, fields(workload.id = request.workload_id))]
    async fn workload_stop(
        &self,
        request: WorkloadStopRequest,
    ) -> anyhow::Result<WorkloadStopResponse> {
        // A cancelled start or stop may still hold its ID and bindings. Wait
        // for or retry its cleanup, then return directly: recovery releases
        // the reservation, leaving no running workload for normal stop handling.
        // What it can leave is the `Error` of a start that had already
        // reported its failure, which this stop collects like any other.
        let recovery = self
            .workload_recoveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&request.workload_id)
            .cloned();
        if let Some(recovery) = recovery {
            // Reported as `Stopping` either way: the id stays held and the
            // teardown keeps being retried, so the workload is stopping whether
            // or not this attempt was the one that finished it. An error here
            // would read as the workload's own state to a caller that then
            // never asks again.
            let message = match recovery
                .recover(
                    &request.workload_id,
                    &self.workloads,
                    &self.workload_recoveries,
                )
                .await
            {
                Ok(()) => "interrupted workload resources released".to_string(),
                Err(error) => {
                    warn!(
                        workload_id = request.workload_id,
                        %error,
                        "interrupted workload cleanup failed again; still retrying"
                    );
                    format!("interrupted workload cleanup failed and is being retried: {error:#}")
                }
            };
            {
                let mut workloads = self.workloads.write().await;
                if matches!(
                    workloads.get(&request.workload_id),
                    Some(HostWorkload::Error(_))
                ) {
                    workloads.remove(&request.workload_id);
                }
            }
            return Ok(WorkloadStopResponse {
                workload_status: WorkloadStatus {
                    workload_id: request.workload_id,
                    workload_state: WorkloadState::Stopping,
                    message,
                },
            });
        }
        let has_workload = self
            .workloads
            .read()
            .await
            .contains_key(&request.workload_id);

        let (workload_state, message) = if has_workload {
            // What a stop can do depends on what it finds, because a workload's
            // map slot is what owns its teardown, and only the owner may write
            // it — otherwise the id frees up mid-teardown and a new workload
            // claiming it is unbound by the old one's `unbind_all_plugins`,
            // which is keyed by workload id.
            //
            // - `Running`: this stop owns it. Mark it under a reservation of
            //   this stop's, tear down, and drop the id.
            // - `Starting` / `Failing`: the start owns it and has not produced a
            //   workload yet. Leave a `Stopping` marker carrying the start's own
            //   reservation; the start sees it, tears down what it built, and
            //   drops the id rather than leaving a plugin's failure behind.
            // - `Stopping`: a teardown is already under way and the id stays
            //   reserved until it finishes. Repeating the stop cannot
            //   help and freeing the id would hand it to a new workload that the
            //   running teardown would then unbind, so this stop reports the
            //   state and leaves the slot alone.
            // - `Error`: nothing is bound (every failure path releases before
            //   recording the error), so the slot can just go.
            let reservation = self.reserve();
            let resolved_workload = {
                let mut workloads = self.workloads.write().await;
                trace!(
                    workload_id = request.workload_id,
                    "updating workload state to stopping"
                );
                // Read what the slot holds before writing it, so the whole
                // decision is one exhaustive match rather than a mutation with
                // an unreachable branch in it.
                let outcome = match workloads.get(&request.workload_id) {
                    // This stop owns the teardown, under a reservation of its
                    // own.
                    Some(HostWorkload::Running(_)) => StopAction::Mark(reservation),
                    // The start owns it. Mark the id under the reservation the
                    // start itself holds, so it recognises the marker as its to
                    // finish.
                    Some(HostWorkload::Starting(held) | HostWorkload::Failing(held, _)) => {
                        StopAction::Mark(*held)
                    }
                    // A teardown is already under way, holding the id until it
                    // is done; nothing here is this stop's to write.
                    Some(HostWorkload::Stopping(_)) => StopAction::Leave,
                    // Nothing is bound, so the slot can just go.
                    Some(HostWorkload::Error(_)) | None => StopAction::Drop,
                };
                match outcome {
                    StopAction::Mark(held) => {
                        match workloads
                            .insert(request.workload_id.clone(), HostWorkload::Stopping(held))
                        {
                            // Only a stop that found the workload running has
                            // anything to tear down here.
                            Some(HostWorkload::Running(rw)) => Some(*rw),
                            _ => None,
                        }
                    }
                    StopAction::Leave => None,
                    StopAction::Drop => {
                        workloads.remove(&request.workload_id);
                        None
                    }
                }
            };

            if let Some(resolved_workload) = resolved_workload {
                // A control attachment may cancel this stop while the host
                // stays alive. Keep its reservation and remaining cleanup
                // together, just as for a cancelled start.
                let mut cleanup_guard = self
                    .workload_cleanup_guard(&request.workload_id, reservation)
                    .with_cleanup(crate::engine::workload::WorkloadResources::for_teardown(
                        &resolved_workload,
                    ));
                debug!(
                    workload_id = request.workload_id,
                    workload_name = resolved_workload.name(),
                    "stopping workload"
                );
                match cleanup_guard.cleanup().release(&request.workload_id).await {
                    Ok(()) => {
                        // Dropped only now, so the id stays reserved for the
                        // whole teardown.
                        self.finish_teardown(&request.workload_id, reservation, None)
                            .await;
                        cleanup_guard.disarm();
                    }
                    // Left armed: dropping the guard hands what is still bound
                    // to a recovery that retries until it succeeds, and a later
                    // stop retries it at once. The workload *is* stopping, so
                    // say so rather than fail: a caller reads an error as the
                    // workload's state and would not ask again.
                    Err(error) => {
                        warn!(
                            workload_id = request.workload_id,
                            %error,
                            "workload cleanup failed; retrying in the background"
                        );
                        return Ok(WorkloadStopResponse {
                            workload_status: WorkloadStatus {
                                workload_id: request.workload_id,
                                workload_state: WorkloadState::Stopping,
                                message: format!(
                                    "workload cleanup failed and is being retried: {error:#}"
                                ),
                            },
                        });
                    }
                }
            }

            debug!(
                workload_id = request.workload_id,
                "workload stopped successfully"
            );

            (
                WorkloadState::Stopping,
                "Workload stopped successfully".to_string(),
            )
        } else {
            (WorkloadState::NotFound, "Workload not found".to_string())
        };

        Ok(WorkloadStopResponse {
            workload_status: WorkloadStatus {
                workload_id: request.workload_id,
                workload_state,
                message,
            },
        })
    }
}

impl WorkloadReservation for Host {
    #[instrument(skip_all, fields(workload.id = workload_id))]
    async fn workload_reserve(&self, workload_id: &str) -> Result<Reservation, String> {
        // Reserve the workload ID while holding the write lock so concurrent
        // starts cannot both observe it as available. An ID remains reserved
        // in every lifecycle state until workload_stop removes it. The
        // reservation stamped on the slot is what lets the commit in
        // `workload_start_reserved` tell this start's slot from one a later
        // start claimed.
        let reservation = self.reserve();
        let mut workloads = self.workloads.write().await;
        if self.stopped.load(std::sync::atomic::Ordering::SeqCst) {
            let message = format!("Host [{}] is stopped and accepts no new workloads", self.id);
            tracing::warn!(workload_id, reason = message, "refused to start workload");
            return Err(message);
        }
        if workloads.contains_key(workload_id) {
            let message = format!(
                "Workload ID [{workload_id}] already exists (the existing workload must be stopped to reuse the ID)"
            );
            // Logged here rather than where the response is built, because this
            // refusal returns before the start ever begins. At `debug`, not
            // `warn`: this is the guard that makes a replayed start request
            // idempotent, a scheduler retrying one is expected and leaves the
            // original workload running, and the refusal is still returned to
            // the caller as an error.
            debug!(workload_id, reason = message, "refused to start workload");
            return Err(message);
        }
        workloads.insert(workload_id.to_string(), HostWorkload::Starting(reservation));
        Ok(reservation)
    }

    #[instrument(skip_all, fields(workload.id = workload_id))]
    async fn workload_release(&self, workload_id: &str, reservation: Reservation) {
        let mut workloads = self.workloads.write().await;
        // Every state this reservation can still be in holds nothing bound: a
        // start that never began, or one a stop or failure handed the teardown
        // back to before it had built anything. Anything else belongs to
        // someone else.
        match workloads.get(workload_id) {
            Some(HostWorkload::Starting(held) | HostWorkload::Stopping(held))
                if *held == reservation =>
            {
                workloads.remove(workload_id);
            }
            Some(HostWorkload::Failing(held, reason)) if *held == reservation => {
                let reason = reason.clone();
                workloads.insert(workload_id.to_string(), HostWorkload::Error(reason));
            }
            _ => {}
        }
    }

    #[instrument(skip_all)]
    async fn workload_start_reserved(
        &self,
        reservation: Reservation,
        request: WorkloadStartRequest,
    ) -> anyhow::Result<WorkloadStartResponse> {
        let workload_id = request.workload_id.clone();
        let mut start_guard = self.workload_cleanup_guard(&workload_id, reservation);
        let started = self
            .workload_start_inner(request, &start_guard.cleanup())
            .await;

        // Commit under the same lock the id was reserved under, and only into
        // the slot this start reserved. Anything else in that slot means the
        // workload was stopped or failed while this was still starting: writing
        // `Running` over it would resurrect a workload nobody is tracking, and
        // simply dropping the result — which is what an `and_modify` that finds
        // no entry does — would leave its plugins bound and its service running
        // detached.
        let (workload_state, message, release_start_resources) = {
            let mut workloads = self.workloads.write().await;
            let slot = workloads.get(&workload_id);
            let mine = matches!(slot, Some(HostWorkload::Starting(held)) if *held == reservation);
            // A plugin failure leaves the start's reservation intact until cleanup.
            let failed_reason = match slot {
                Some(HostWorkload::Failing(held, reason)) if *held == reservation => {
                    Some(reason.clone())
                }
                _ => None,
            };
            match started {
                Ok(mut resolved) if mine => {
                    start_guard.cleanup().commit(&mut resolved);
                    start_guard.disarm();
                    workloads.insert(
                        workload_id.clone(),
                        HostWorkload::Running(Box::new(resolved)),
                    );
                    (
                        WorkloadState::Running,
                        "Workload started successfully".to_string(),
                        false,
                    )
                }
                // Stopped (or failed) while starting. The stop could not tear
                // this down because it did not exist yet, so that falls to us.
                // The `Stopping` or `Failing` marker remains for the whole teardown:
                // it keeps the id reserved, so a new start cannot claim it and
                // then be unbound by our teardown.
                Ok(_) => match failed_reason {
                    Some(reason) => (WorkloadState::Error, reason, true),
                    None => (
                        WorkloadState::Stopping,
                        "Workload was stopped while starting".to_string(),
                        true,
                    ),
                },
                Err(err) => {
                    // `{:#}` so the whole context chain reaches the caller and
                    // the log below: the outer layer alone ("failed to pull
                    // image for component 'x'") never names the cause.
                    let message = format!("{err:#}");
                    if mine {
                        workloads.insert(
                            workload_id.clone(),
                            HostWorkload::Failing(reservation, message.clone()),
                        );
                    }
                    (WorkloadState::Error, message, true)
                }
            }
        };

        // A start that failed is reported to whoever asked and nowhere else.
        // Say so locally too: the operator diagnosing it — a pull against a
        // registry this host does not trust, a plugin that would not bind — is
        // reading the host's log, and without this the host has nothing to say.
        if workload_state == WorkloadState::Error {
            // `reason`, not `message`: `message` is the field tracing gives
            // the event's own text, and a second one under that name displaces
            // it.
            tracing::error!(workload_id, reason = message, "failed to start workload");
        }

        if release_start_resources {
            match start_guard.cleanup().release(&workload_id).await {
                Ok(()) => {
                    self.finish_teardown(&workload_id, reservation, None).await;
                    start_guard.disarm();
                }
                // Keep the guard armed so unfinished cleanup stays retryable by a stop.
                // The caller is still told what failed, so recovery publishes
                // that reason rather than dropping it with the id.
                Err(error) => {
                    warn!(workload_id, %error, "failed to release unfinished workload start");
                    start_guard.retain_failure();
                }
            }
        }

        Ok(WorkloadStartResponse {
            workload_status: WorkloadStatus {
                workload_id,
                workload_state,
                message,
            },
        })
    }
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("id", &self.id)
            .field("hostname", &self.hostname)
            .field("friendly_name", &self.friendly_name)
            .field("environment", &self.environment)
            .field("version", &self.version)
            .field("labels", &self.labels)
            .field("started_at", &self.started_at)
            .field("workloads", &self.workloads)
            .finish()
    }
}

/// Drain plugin-reported workload failures for the lifetime of the host,
/// transitioning each reported workload to a failed state. Ends when the host
/// is dropped, or when the last [`WorkloadFailureSink`] is (a host with no
/// plugin that keeps one).
///
/// The host is held weakly and upgraded per report so this task never keeps it
/// alive — see the spawn site in [`Host::start`].
async fn consume_workload_failures(
    host: std::sync::Weak<Host>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<WorkloadFailure>,
) {
    while let Some(WorkloadFailure {
        workload_id,
        reason,
    }) = rx.recv().await
    {
        let Some(host) = host.upgrade() else {
            break;
        };
        host.fail_workload(&workload_id, reason).await;
    }
}

/// Config for the [`Host`]
#[derive(Clone, Debug)]
pub struct HostConfig {
    pub allow_oci_insecure: bool,
    pub oci_pull_timeout: Option<Duration>,
    pub oci_cache_dir: Option<PathBuf>,
    /// PEM CA bundles to trust for OCI pulls, on top of the OS trust store and
    /// `SSL_CERT_FILE` / `SSL_CERT_DIR`. Needed to reach a registry behind a
    /// private CA those do not cover — an in-cluster one, or a corporate mirror.
    ///
    /// Applied by [`HostBuilder::build`] to the process-wide trust store, so an
    /// embedder that fills in this struct configures registry trust the same
    /// way it configures every other OCI setting. `wash oci pull`/`push`, which
    /// build no host at all, reach that store through
    /// [`oci::set_extra_ca_certificates`](crate::oci::set_extra_ca_certificates)
    /// directly.
    ///
    /// That store holds one set for the whole process. Two hosts in one process
    /// may name the same bundles; a second host naming *different* ones fails
    /// to build, rather than running on trust it did not ask for.
    pub oci_ca_paths: Vec<PathBuf>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            allow_oci_insecure: false,
            oci_pull_timeout: Duration::from_secs(30).into(),
            oci_cache_dir: None,
            oci_ca_paths: Vec::new(),
        }
    }
}

/// Builder for the [`Host`]
pub struct HostBuilder {
    id: String,
    engine: Option<Engine>,
    plugins: HashMap<&'static str, Arc<dyn HostPlugin>>,
    plugin_bindings: crate::plugin::PluginBindings,
    hostname: Option<String>,
    friendly_name: Option<String>,
    environment: Option<String>,
    labels: HashMap<String, String>,
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
    lifetime: Arc<HostLifetime>,
    config: Option<HostConfig>,
    /// Unset until [`HostBuilder::with_meters`], resolved in
    /// [`HostBuilder::build`]. An option because [`Meters::new`] binds its
    /// instruments to the OTel provider global *at that moment*, and one built
    /// before `set_meter_provider` is a no-op for good.
    meters: Option<Meters>,
}

impl Default for HostBuilder {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            engine: Default::default(),
            plugins: Default::default(),
            plugin_bindings: Default::default(),
            hostname: Default::default(),
            friendly_name: Default::default(),
            environment: Default::default(),
            labels: Default::default(),
            http_handler: Default::default(),
            lifetime: Arc::new(HostLifetime(())),
            config: Default::default(),
            meters: Default::default(),
        }
    }
}

impl HostBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn with_engine(mut self, engine: Engine) -> Self {
        self.engine = Some(engine);
        self
    }

    /// Overrides the default HTTP handler.
    pub fn with_http_handler(mut self, handler: Arc<dyn crate::host::http::HostHandler>) -> Self {
        self.http_handler = Some(handler);
        self
    }

    pub fn with_plugin(mut self, plugin: Arc<dyn HostPlugin>) -> anyhow::Result<Self> {
        let plugin_id = plugin.id();

        // Check for duplicate plugin IDs
        if self.plugins.contains_key(plugin_id) {
            bail!("Duplicate plugin ID '{plugin_id}' - plugin IDs must be unique");
        }

        self.plugins.insert(plugin_id, plugin);
        Ok(self)
    }

    /// Sets what the operator declared about the registered plugins' bindings.
    ///
    /// Checked against the registered plugins by [`HostBuilder::build`], so
    /// call this at any point before it — a declaration naming a plugin the
    /// host does not have is refused rather than left inert.
    pub fn with_plugin_bindings(mut self, bindings: crate::plugin::PluginBindings) -> Self {
        self.plugin_bindings = bindings;
        self
    }

    /// Every native (non-component) plugin registered so far — what a host
    /// component plugin's own capability imports resolve against
    /// ([`crate::plugin::component_host::load_component_plugin`]). Excludes
    /// any component plugin already registered, so a loading plugin can never
    /// import from another component plugin, only from host natives.
    ///
    /// A snapshot, not a live view: call this (and [`Self::http_handler`])
    /// only after every native plugin and the HTTP handler are registered,
    /// then load host component plugins last. Registering a native or
    /// calling [`Self::with_http_handler`] afterward has no effect on a
    /// component plugin already loaded from an earlier snapshot — a missing
    /// native fails loudly at that plugin's construction (an unresolved
    /// import), but a missing HTTP handler fails silently until the plugin's
    /// first outbound call traps with "http client not available".
    #[cfg(feature = "host-component-plugins")]
    pub fn native_plugins(&self) -> HashMap<&'static str, Arc<dyn HostPlugin>> {
        crate::plugin::component_host::native_only(&self.plugins)
    }

    /// The HTTP handler registered so far, if any — what a host component
    /// plugin's own `wasi:http/outgoing-handler` calls are sent through, the
    /// same handler a workload's outgoing calls use.
    ///
    /// A snapshot, not a live view — see [`Self::native_plugins`]'s doc for
    /// the ordering this requires and the silent-until-first-call failure
    /// mode of getting it wrong.
    #[cfg(feature = "host-component-plugins")]
    pub fn http_handler(&self) -> Option<Arc<dyn crate::host::http::HostHandler>> {
        self.http_handler.clone()
    }

    /// Reference for a plugin built before this builder becomes a host.
    #[cfg(feature = "host-component-plugins")]
    pub fn host_ref(&self) -> HostRef {
        HostRef {
            handler: self.http_handler.as_ref().map(Arc::downgrade),
            lifetime: Some(Arc::downgrade(&self.lifetime)),
        }
    }

    /// Registers the multiplexed plugin set from
    /// [`crate::plugin::multiplexed_plugins`], which is what makes
    /// `(implements ..)` named imports resolvable. Without it, every
    /// registered plugin reports `supports_named_instances() == false` and a
    /// workload needing named multiplexing fails to bind.
    #[cfg(feature = "wasm_component_model_implements")]
    pub fn with_multiplexed_plugins(mut self) -> anyhow::Result<Self> {
        for plugin in crate::plugin::multiplexed_plugins() {
            self = self.with_plugin(plugin)?;
        }
        Ok(self)
    }

    pub fn with_meters(mut self, meters: Meters) -> Self {
        self.meters = Some(meters);
        self
    }

    /// Sets the hostname for this host.
    ///
    /// # Arguments
    /// * `hostname` - The hostname to use
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_hostname(mut self, hostname: impl AsRef<str>) -> Self {
        self.hostname = Some(hostname.as_ref().to_string());
        self
    }

    /// Sets a human-readable friendly name for this host.
    ///
    /// # Arguments
    /// * `name` - The friendly name to use
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_friendly_name(mut self, name: impl AsRef<str>) -> Self {
        self.friendly_name = Some(name.as_ref().to_string());
        self
    }

    /// Sets the environment this host advertises itself as running in.
    ///
    /// For Kubernetes host pods this is typically the pod's namespace
    /// (sourced via the downward API and passed as `--environment`); for
    /// out-of-cluster hosts it can be any string identifying where the
    /// host runs (e.g. a region or data center).
    ///
    /// # Arguments
    /// * `environment` - The environment string to advertise
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_environment(mut self, environment: impl AsRef<str>) -> Self {
        self.environment = Some(environment.as_ref().to_string());
        self
    }

    /// Adds a label to the host.
    ///
    /// Labels are key-value pairs that can be used to categorize
    /// or identify the host.
    ///
    /// # Arguments
    /// * `key` - The label key
    /// * `value` - The label value
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_label(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        self.labels
            .insert(key.as_ref().to_string(), value.as_ref().to_string());
        self
    }

    pub fn with_config(mut self, config: HostConfig) -> Self {
        self.config.replace(config);
        self
    }

    /// Builds and returns a configured [`Host`].
    ///
    /// This method finalizes the configuration and creates the host.
    /// If no engine is provided, a default engine is created.
    /// If no hostname is provided, the system hostname is used.
    /// If no friendly name is provided, a random name is generated.
    ///
    /// # Returns
    /// A new `Host` instance ready to be started.
    ///
    /// # Errors
    /// Returns an error if the default engine cannot be created (when no engine
    /// is provided), if a CA bundle named by [`HostConfig::oci_ca_paths`]
    /// cannot be read or does not parse, or if a different set of bundles is
    /// already configured for this process.
    pub fn build(self) -> anyhow::Result<Host> {
        let config = self.config.unwrap_or_default();

        // Trust roots first, before anything this host builds can pull: the
        // host pulls on behalf of every workload, and a bundle that cannot be
        // read is a startup failure rather than a registry that rejects every
        // pull much later. A host built without `oci` has no registry client
        // for them to apply to.
        #[cfg(feature = "oci")]
        crate::oci::set_extra_ca_certificates(&config.oci_ca_paths)
            .context("failed to load the OCI CA certificates in the host config")?;

        let engine = if let Some(engine) = self.engine {
            engine
        } else {
            Engine::builder().build()?
        };

        // Resolved here, not in `HostBuilder::default`, so the histograms come
        // from the meter provider an embedder installed rather than whatever
        // was global when it started building.
        let mut meters = self.meters.unwrap_or_default();

        // Fuel is the one meter the engine has to cooperate on, and nothing
        // makes the two knobs agree. Either way round costs something the
        // operator did not ask for, so say which way it went.
        match (meters.fuel_consumption.is_enabled(), engine.consumes_fuel()) {
            (true, false) => {
                warn!(
                    "fuel metering was asked for, but this host's engine compiles no fuel \
                     counters; recording invocation duration only. Pass the same choice to both \
                     `EngineBuilder::with_fuel_consumption` and `HostBuilder::with_meters`"
                );
                meters.fuel_consumption = FuelConsumptionMeter::new(false);
            }
            (false, true) => warn!(
                "this host's engine compiles fuel counters into every guest, but no meter reads \
                 them. Guests pay for the counting either way — pass the same choice to both \
                 `EngineBuilder::with_fuel_consumption` and `HostBuilder::with_meters`"
            ),
            (true, true) | (false, false) => {}
        }

        // Get hostname from system if not provided
        let hostname = self.hostname.unwrap_or_else(|| {
            hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        });

        // Generate a friendly name if not provided
        let friendly_name = self.friendly_name.unwrap_or_else(|| {
            let mut generator = Generator::with_naming(Name::Numbered);
            generator
                .next()
                .unwrap_or_else(|| format!("host-{}", uuid::Uuid::new_v4()))
        });

        // Use a null HTTP handler if none provided
        // It will reject any HTTP requests
        let http_handler = match self.http_handler {
            Some(handler) => handler,
            None => Arc::new(crate::host::http::NullServer::default()),
        };
        let port_reservations = reserve_http_ingress(&engine.socket_policy, http_handler.port())?;

        // Every plugin is registered by now, so a binding declaration naming an
        // id this host does not have is a typo — and an inert one, which is the
        // dangerous shape: a `workloadConfig: deny` that never applies.
        let registered: Vec<&str> = self.plugins.keys().copied().collect();
        self.plugin_bindings.validate_against(&registered)?;

        // Each plugin checks its own declaration with its own parser, so a
        // binding an operator wrote wrong fails startup rather than the first
        // workload that names it.
        for id in self.plugin_bindings.plugin_ids() {
            // Skipped, not refused: `validate_against` already warned that this
            // build has no such plugin. Nothing can bind it either, so the
            // declaration is inert rather than wrong.
            let Some(plugin) = self.plugins.get(id) else {
                continue;
            };
            let declared = self.plugin_bindings.for_plugin(id);
            declared.validate_declaration()?;
            if let Some(policy) = declared.egress_policy(&engine.socket_policy) {
                plugin
                    .configure_egress_policy(policy)
                    .with_context(|| format!("invalid egress policy for host plugin '{id}'"))?;
            }
            // The schema check next: an operator typo is named as a typo,
            // rather than as whatever the plugin's parser makes of a config
            // missing the key they meant to set.
            declared.reject_unknown_keys(&plugin.binding_schema())?;
            plugin
                .validate_bindings(&declared)
                .with_context(|| format!("invalid `host.plugins` declaration for '{id}'"))?;
        }

        Ok(Host {
            engine,
            workloads: Arc::default(),
            workload_recoveries: Arc::default(),
            workload_cleanup_tasks: {
                let tasks = tokio_util::task::TaskTracker::new();
                tasks.close();
                tasks
            },
            reservations: std::sync::atomic::AtomicU64::default(),
            stopped: std::sync::atomic::AtomicBool::new(false),
            #[cfg(feature = "washlet")]
            control: std::sync::Mutex::default(),
            plugins: self.plugins,
            plugin_bindings: Arc::new(self.plugin_bindings),
            id: self.id,
            hostname,
            friendly_name,
            environment: self.environment.unwrap_or_default(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            labels: self.labels,
            started_at: chrono::Utc::now(),
            system_monitor: Arc::new(RwLock::new(SystemMonitor::new())),
            http_handler,
            _port_reservations: port_reservations,
            lifetime: self.lifetime,
            config,
            meters,
        })
    }
}

fn reserve_http_ingress(
    policy: &crate::sockets::policy::SocketPolicy,
    port: u16,
) -> anyhow::Result<Vec<crate::host::ports::PortReservation>> {
    let Some(table) = policy.host_owned_ports.as_ref().filter(|_| port != 0) else {
        return Ok(Vec::new());
    };
    let owner = crate::host::ports::PortOwner::Host("HTTP ingress".into());
    [
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::net::SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
    ]
    .into_iter()
    .map(|addr| {
        table.reserve(
            crate::host::declared_port::Protocol::Tcp,
            addr,
            owner.clone(),
        )
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Component;

    #[test]
    fn http_ingress_is_host_owned() {
        let policy = crate::sockets::policy::SocketPolicy {
            host_loopback_enabled: true,
            host_loopback: Arc::from([crate::host::allowed_loopback::AllowedLoopbackPort::tcp(
                8000,
            )]),
            // The table comes from the host that owns it, as it does in `wash
            // host` and `wash dev`: without one there is nothing to reserve
            // into, and every port reads as unowned.
            host_owned_ports: Some(crate::host::ports::PortTable::new()),
            ..Default::default()
        };
        let _reservations = reserve_http_ingress(&policy, 8000).unwrap();
        assert!(matches!(
            policy.decide(
                crate::sockets::SocketAddrUse::TcpConnect,
                "127.255.255.254:8000".parse().unwrap()
            ),
            crate::sockets::AddrDecision::Deny(crate::sockets::DenyReason::HostOwnedPort)
        ));
    }

    /// An unreadable CA bundle has to stop the host being built. Trust is
    /// configured once and used much later, so accepting it here would surface
    /// as every pull from that registry failing to verify, far from the typo
    /// that caused it.
    #[cfg(feature = "oci")]
    #[test]
    fn an_unreadable_oci_ca_bundle_fails_the_build() {
        let err = Host::builder()
            .with_config(HostConfig {
                oci_ca_paths: vec![PathBuf::from("/definitely/not/a/ca.pem")],
                ..Default::default()
            })
            .build()
            .expect_err("a host must not build around a CA bundle it cannot read");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("/definitely/not/a/ca.pem"),
            "the error should name the bundle it could not read: {msg}"
        );
    }

    /// These assertions only read `host.meters`, so they have no use for a
    /// pooling allocator's address-space reservation.
    fn frugal_engine(fuel: bool) -> Engine {
        Engine::builder()
            .with_pooling_allocator(false)
            .with_fuel_consumption(fuel)
            .build()
            .expect("a minimal engine must build")
    }

    /// What [`crate::engine::abandon::arm_epoch_deadline`] relies on to end a
    /// guest whose host is gone. The handler cannot answer this: an embedder
    /// that keeps its own clone — as `tests/common` does, to read the bound
    /// address back — would keep every torn-down host looking alive.
    #[test]
    fn a_store_sees_its_host_go_away_though_a_handler_clone_remains() {
        let handler: Arc<dyn crate::host::http::HostHandler> =
            Arc::new(crate::host::http::NullServer::default());
        let host = Host::builder()
            .with_engine(frugal_engine(false))
            .with_http_handler(Arc::clone(&handler))
            .build()
            .expect("a minimal host must build");
        let link = crate::engine::ctx::Ctx::builder("wk", "comp")
            .with_host(&host.host_ref())
            .build()
            .host_link();

        assert!(!link.host_is_gone(), "the host is right here");
        drop(host);
        assert!(
            link.host_is_gone(),
            "the host is gone, and the handler clone this embedder kept must \
             not make its store look like it is still hosted"
        );
        assert!(
            Arc::strong_count(&handler) > 0,
            "precondition: the embedder's handler clone outlives the host"
        );
    }

    #[cfg(feature = "host-component-plugins")]
    #[test]
    fn a_plugin_store_sees_its_host_go_away() {
        let handler: Arc<dyn crate::host::http::HostHandler> =
            Arc::new(crate::host::http::NullServer::default());
        let builder = Host::builder()
            .with_engine(frugal_engine(false))
            .with_http_handler(Arc::clone(&handler));
        let host_ref = builder.host_ref();
        let link = crate::engine::ctx::Ctx::builder("plugin", "plugin")
            .with_host(&host_ref)
            .build()
            .host_link();

        let host = builder.build().expect("a minimal host must build");
        assert!(!link.host_is_gone());
        assert!(host_ref.handler().is_some());
        drop(host);
        assert!(link.host_is_gone());
        assert!(host_ref.handler().is_none());
        assert!(Arc::strong_count(&handler) > 0);
    }

    #[cfg(feature = "host-component-plugins")]
    #[test]
    fn a_plugin_store_without_egress_sees_its_host_go_away() {
        let builder = Host::builder().with_engine(frugal_engine(false));
        let host_ref = builder.host_ref();
        let link = crate::engine::ctx::Ctx::builder("plugin", "plugin")
            .with_host(&host_ref)
            .build()
            .host_link();

        let host = builder.build().expect("a minimal host must build");
        assert!(!link.host_is_gone());
        drop(host);
        assert!(link.host_is_gone());
    }

    /// An embedder that never calls `with_meters` gets what
    /// [`MeterKind`](crate::observability::MeterKind) says the default is, not
    /// silence. Nothing else exercises it: `wash host` and `wash dev` both pass
    /// their `--meters` choice explicitly, so only embedders reach the default.
    #[test]
    fn a_host_built_without_meters_measures_the_default() {
        let host = Host::builder()
            .with_engine(frugal_engine(false))
            .build()
            .expect("a host with nothing configured must build");
        assert!(
            host.meters.invocation.is_enabled(),
            "a host built without `with_meters` must measure what `MeterKind::default()` names"
        );
        assert!(
            !host.meters.fuel_consumption.is_enabled(),
            "the default is `Duration`; fuel costs the guest and must stay opt-in"
        );
    }

    /// An explicit choice still wins, including the one that measures nothing.
    #[test]
    fn with_meters_overrides_the_default() {
        let host = Host::builder()
            .with_engine(frugal_engine(false))
            .with_meters(Meters::new(crate::observability::MeterKind::Off))
            .build()
            .expect("a host metering nothing must build");
        assert!(
            !host.meters.invocation.is_enabled(),
            "`MeterKind::Off` asked for no measurement and must get none"
        );
    }

    /// A fuel meter an engine cannot answer is dropped rather than kept: the
    /// duration half of the same kind still records.
    #[test]
    fn fuel_metering_gives_way_to_an_engine_that_counts_no_fuel() {
        let host = Host::builder()
            .with_engine(frugal_engine(false))
            .with_meters(Meters::new(crate::observability::MeterKind::Fuel))
            .build()
            .expect("a host asking for fuel on a fuel-less engine must still build");
        assert!(
            !host.meters.fuel_consumption.is_enabled(),
            "a fuel meter belongs only on an engine that compiles fuel counters"
        );
        assert!(
            host.meters.invocation.is_enabled(),
            "giving up fuel must not give up the duration the same kind asked for"
        );
    }

    /// The pair an embedder has to set together, set together.
    #[test]
    fn fuel_metering_survives_an_engine_that_counts_fuel() {
        let host = Host::builder()
            .with_engine(frugal_engine(true))
            .with_meters(Meters::new(crate::observability::MeterKind::Fuel))
            .build()
            .expect("a host must build on a fuel-consuming engine");
        assert!(
            host.meters.fuel_consumption.is_enabled(),
            "an engine that counts fuel must keep the meter that reads it"
        );
    }

    /// A `Meters` the caller kept a copy of must not fail calls either: the
    /// builder can only repair the copy it consumed, so the guard that matters
    /// is the one in `FuelConsumptionMeter::observe`.
    #[tokio::test]
    async fn a_retained_fuel_meter_still_runs_calls_on_a_fuel_less_engine() {
        let engine = frugal_engine(false);
        let meters = Meters::new(crate::observability::MeterKind::Fuel);
        let _host = Host::builder()
            .with_engine(engine.clone())
            .with_meters(meters.clone())
            .build()
            .expect("a host must build");

        let ctx = crate::engine::ctx::Ctx::builder("workload", "component").build();
        let mut store =
            wasmtime::Store::new(engine.inner(), crate::engine::ctx::SharedCtx::new(ctx));
        let measured = meters
            .guest()
            .observe(&[], &mut store, async |_| Ok(7))
            .await
            .expect("a call must not fail because its fuel cannot be read");
        assert_eq!(measured, 7, "the measured call's own value must come back");
    }

    fn empty_workload_start_request(workload_id: &str) -> WorkloadStartRequest {
        WorkloadStartRequest {
            workload_id: workload_id.to_string(),
            workload: Workload {
                namespace: "wasmcloud".to_string(),
                name: "empty".to_string(),
                annotations: Default::default(),
                service: None,
                components: vec![],
                host_interfaces: vec![],
                volumes: vec![],
            },
        }
    }

    /// `Host::stop`'s per-plugin timeout must outlast the plugin-stop budget.
    /// A host component plugin's `stop()` waits the full budget for its
    /// supervisor and only then runs its abort-and-cleanup tail; if the outer
    /// timeout fired first it would drop that future mid-wait, detaching the
    /// supervisor's JoinHandle instead of aborting it. The mock below mirrors
    /// that shape: sleep the budget, then record that the tail ran.
    #[tokio::test(start_paused = true)]
    async fn test_stop_outlasts_plugin_stop_budget() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct SlowStopPlugin {
            cleanup_ran: Arc<AtomicBool>,
        }

        #[async_trait::async_trait]
        impl HostPlugin for SlowStopPlugin {
            fn id(&self) -> &'static str {
                "slow-stop"
            }

            fn world(&self) -> WitWorld {
                WitWorld::default()
            }

            async fn stop(&self) -> anyhow::Result<()> {
                tokio::time::sleep(crate::timeouts::plugin_stop()).await;
                self.cleanup_ran.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        let cleanup_ran = Arc::new(AtomicBool::new(false));
        let host = Host::builder()
            .with_plugin(Arc::new(SlowStopPlugin {
                cleanup_ran: Arc::clone(&cleanup_ran),
            }))
            .expect("failed to register plugin")
            .build()
            .expect("failed to build host");

        Arc::new(host).stop().await.expect("failed to stop host");

        assert!(
            cleanup_ran.load(Ordering::SeqCst),
            "plugin stop's post-budget cleanup must run before Host::stop gives up on it"
        );
    }

    #[tokio::test]
    async fn test_workload_start_rejects_existing_id() {
        let host = Host::builder().build().expect("failed to build host");
        let request = empty_workload_start_request("duplicate");

        let first = host
            .workload_start(request.clone())
            .await
            .expect("first start should return a response");
        assert_eq!(first.workload_status.workload_state, WorkloadState::Running);

        let duplicate = host
            .workload_start(request)
            .await
            .expect("duplicate start should return a response");
        assert_eq!(
            duplicate.workload_status.workload_state,
            WorkloadState::Error
        );
        assert!(
            duplicate
                .workload_status
                .message
                .contains("Workload ID [duplicate] already exists"),
            "unexpected rejection message: {}",
            duplicate.workload_status.message
        );

        let workloads = host.workloads.read().await;
        assert_eq!(workloads.len(), 1);
        assert!(matches!(
            workloads.get("duplicate"),
            Some(HostWorkload::Running(_))
        ));
    }

    #[tokio::test]
    async fn test_concurrent_workload_starts_reserve_id_atomically() {
        let host = Host::builder().build().expect("failed to build host");
        let request = empty_workload_start_request("concurrent-duplicate");

        let (first, second) = tokio::join!(
            host.workload_start(request.clone()),
            host.workload_start(request)
        );
        let states = [
            first
                .expect("first start should return a response")
                .workload_status
                .workload_state,
            second
                .expect("second start should return a response")
                .workload_status
                .workload_state,
        ];

        assert_eq!(
            states
                .iter()
                .filter(|state| **state == WorkloadState::Running)
                .count(),
            1
        );
        assert_eq!(
            states
                .iter()
                .filter(|state| **state == WorkloadState::Error)
                .count(),
            1
        );
        assert_eq!(host.workloads.read().await.len(), 1);
    }

    #[tokio::test]
    async fn test_workload_start_failed() {
        let host = Host::builder().build().expect("failed to build host");

        let workload_status = host
            .workload_start(WorkloadStartRequest {
                workload_id: "test".to_string(),
                workload: Workload {
                    namespace: "wasmcloud".to_string(),
                    name: "test".to_string(),
                    annotations: Default::default(),
                    service: None,
                    components: vec![Component {
                        name: "test".to_string(),
                        digest: None,
                        bytes: vec![0xD, 0xE, 0xA, 0xD, 0xB, 0xE, 0xE, 0xF].into(),
                        local_resources: Default::default(),
                        pool_size: 1,
                        max_invocations: 100,
                        max_concurrency: 1,
                        ..Default::default()
                    }],
                    host_interfaces: vec![],
                    volumes: vec![],
                },
            })
            .await;

        assert!(matches!(
            workload_status,
            Ok(WorkloadStartResponse {
                workload_status: WorkloadStatus {
                    workload_state: WorkloadState::Error,
                    ..
                }
            })
        ));
    }

    #[tokio::test]
    async fn test_service_start_failed_with_invalid_wasm() {
        let host = Host::builder().build().expect("failed to build host");

        let workload_status = host
            .workload_start(WorkloadStartRequest {
                workload_id: "test-bad-service".to_string(),
                workload: Workload {
                    namespace: "wasmcloud".to_string(),
                    name: "bad-service-test".to_string(),
                    annotations: Default::default(),
                    service: Some(crate::types::Service {
                        bytes: vec![0xDE, 0xAD, 0xBE, 0xEF].into(),
                        digest: None,
                        local_resources: Default::default(),
                        max_restarts: 0,
                    }),
                    components: vec![],
                    host_interfaces: vec![],
                    volumes: vec![],
                },
            })
            .await;

        assert!(matches!(
            workload_status,
            Ok(WorkloadStartResponse {
                workload_status: WorkloadStatus {
                    workload_state: WorkloadState::Error,
                    ..
                }
            })
        ));
    }

    /// Records which workloads it was bound to and unbound from, so a test can
    /// assert that a workload which never finished starting still gave its
    /// binding back. `bind_delay` holds a start open long enough for a stop to
    /// race it, and `unbind_delay` does the same for a teardown.
    #[derive(Default)]
    struct BindRecordingPlugin {
        bound: std::sync::Mutex<Vec<String>>,
        unbound: std::sync::Mutex<Vec<String>>,
        bind_delay: Duration,
        unbind_delay: Duration,
        unbind_entered: tokio::sync::Notify,
        unbind_gate: Option<Arc<tokio::sync::Semaphore>>,
    }

    impl BindRecordingPlugin {
        fn unbound(&self) -> Vec<String> {
            self.unbound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn bound(&self) -> Vec<String> {
            self.bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl HostPlugin for BindRecordingPlugin {
        fn id(&self) -> &'static str {
            "bind-recording"
        }

        fn world(&self) -> WitWorld {
            WitWorld {
                imports: HashSet::from([WitInterface::from("test:probe/marker@0.1.0")]),
                exports: HashSet::new(),
            }
        }

        async fn on_workload_bind(
            &self,
            workload: &crate::engine::workload::UnresolvedWorkload,
            _interfaces: crate::plugin::WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            if !self.bind_delay.is_zero() {
                tokio::time::sleep(self.bind_delay).await;
            }
            self.bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(workload.id().to_string());
            Ok(())
        }

        async fn on_workload_unbind(
            &self,
            workload_id: &str,
            _interfaces: crate::plugin::WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            self.unbind_entered.notify_one();
            if let Some(gate) = &self.unbind_gate {
                gate.acquire().await?.forget();
            }
            if !self.unbind_delay.is_zero() {
                tokio::time::sleep(self.unbind_delay).await;
            }
            self.unbound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(workload_id.to_string());
            Ok(())
        }
    }

    /// A component whose only import is the interface `BindRecordingPlugin`
    /// serves, so a workload carrying it binds that plugin.
    fn marker_component_wasm() -> Vec<u8> {
        wat::parse_str(r#"(component (import "test:probe/marker@0.1.0" (instance)))"#)
            .expect("failed to parse WAT")
    }

    /// The same, plus a single exported interface — enough to pass the
    /// service's export validation, but not `wasi:cli/run`, so building a
    /// command from it fails. That failure lands *after* `resolve` has bound
    /// the workload's plugins, which is the window these tests exercise.
    fn marker_service_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"(component
                  (import "test:probe/marker@0.1.0" (instance))
                  (instance $empty)
                  (export "test:probe/runner@0.1.0" (instance $empty))
               )"#,
        )
        .expect("failed to parse WAT")
    }

    fn marker_interfaces() -> Vec<WitInterface> {
        vec![WitInterface::from("test:probe/marker@0.1.0")]
    }

    /// A workload whose service binds the plugin and then fails to start: the
    /// component is valid and resolves, so plugins bind, but it exports no
    /// `wasi:cli/run` for the service driver to call.
    fn service_fails_after_bind_request(workload_id: &str) -> WorkloadStartRequest {
        WorkloadStartRequest {
            workload_id: workload_id.to_string(),
            workload: Workload {
                namespace: "wasmcloud".to_string(),
                name: "binds-then-fails".to_string(),
                annotations: Default::default(),
                service: Some(crate::types::Service {
                    bytes: marker_service_wasm().into(),
                    digest: None,
                    local_resources: Default::default(),
                    max_restarts: 0,
                }),
                components: vec![],
                host_interfaces: marker_interfaces(),
                volumes: vec![],
            },
        }
    }

    /// A workload of one plain component that binds the plugin and reaches
    /// `Running` — what a stop finds when it owns the teardown itself.
    fn marker_request(workload_id: &str) -> WorkloadStartRequest {
        WorkloadStartRequest {
            workload_id: workload_id.to_string(),
            workload: Workload {
                namespace: "wasmcloud".to_string(),
                name: workload_id.to_string(),
                annotations: Default::default(),
                service: None,
                components: vec![Component {
                    name: "marker".to_string(),
                    digest: None,
                    bytes: marker_component_wasm().into(),
                    local_resources: Default::default(),
                    pool_size: 1,
                    max_invocations: 100,
                    max_concurrency: 1,
                    ..Default::default()
                }],
                host_interfaces: marker_interfaces(),
                volumes: vec![],
            },
        }
    }

    fn host_with(plugin: Arc<BindRecordingPlugin>) -> Host {
        Host::builder()
            .with_plugin(plugin)
            .expect("failed to register plugin")
            .build()
            .expect("failed to build host")
    }

    #[derive(Clone, Copy)]
    enum CancelAt {
        WorkloadBind,
        ItemBind,
        Resolved,
    }

    struct CancelledBindingPlugin {
        cancel_at: CancelAt,
        entered: tokio::sync::Notify,
        cleanup_entered: tokio::sync::Notify,
        cleanup_gate: tokio::sync::Semaphore,
        live: std::sync::atomic::AtomicBool,
        fail_cleanup: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl HostPlugin for CancelledBindingPlugin {
        fn id(&self) -> &'static str {
            "cancelled-binding"
        }

        fn world(&self) -> WitWorld {
            WitWorld {
                imports: HashSet::new(),
                exports: HashSet::from([WitInterface::from("test:probe/marker@0.1.0")]),
            }
        }

        async fn on_workload_bind(
            &self,
            _workload: &crate::engine::workload::UnresolvedWorkload,
            _interfaces: crate::plugin::WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            self.live.store(true, std::sync::atomic::Ordering::SeqCst);
            if matches!(self.cancel_at, CancelAt::WorkloadBind) {
                self.entered.notify_one();
                std::future::pending::<()>().await;
            }
            Ok(())
        }

        async fn on_workload_item_bind<'a>(
            &self,
            item: &mut crate::engine::workload::WorkloadItem<'a>,
            _interfaces: crate::plugin::WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            if matches!(self.cancel_at, CancelAt::ItemBind) {
                self.entered.notify_one();
                std::future::pending::<()>().await;
            }
            item.linker().instance("test:probe/marker@0.1.0")?;
            Ok(())
        }

        async fn on_workload_resolved(
            &self,
            _workload: &crate::engine::workload::ResolvedWorkload,
            _component_id: &str,
        ) -> anyhow::Result<()> {
            if matches!(self.cancel_at, CancelAt::Resolved) {
                self.entered.notify_one();
                std::future::pending::<()>().await;
            }
            Ok(())
        }

        async fn on_workload_unbind(
            &self,
            _workload_id: &str,
            interfaces: crate::plugin::WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            assert!(interfaces.contains("test", "probe", &["marker"]));
            self.cleanup_entered.notify_one();
            let permit = self.cleanup_gate.acquire().await?;
            permit.forget();
            anyhow::ensure!(
                !self
                    .fail_cleanup
                    .swap(false, std::sync::atomic::Ordering::SeqCst),
                "cleanup failed once"
            );
            self.live.store(false, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn cancelled_native_starts_keep_the_id_until_binding_cleanup_finishes()
    -> anyhow::Result<()> {
        for cancel_at in [
            CancelAt::WorkloadBind,
            CancelAt::ItemBind,
            CancelAt::Resolved,
        ] {
            let plugin = Arc::new(CancelledBindingPlugin {
                cancel_at,
                entered: tokio::sync::Notify::new(),
                cleanup_entered: tokio::sync::Notify::new(),
                cleanup_gate: tokio::sync::Semaphore::new(0),
                live: std::sync::atomic::AtomicBool::new(false),
                fail_cleanup: std::sync::atomic::AtomicBool::new(false),
            });
            let host = Host::builder()
                .with_plugin(plugin.clone())?
                .build()?
                .start()
                .await?;
            host.workload_start(empty_workload_start_request("local"))
                .await?;
            #[cfg(feature = "washlet")]
            let lease = host.acquire_control()?;
            let task = tokio::spawn({
                let host = host.clone();
                async move { host.workload_start(marker_request("cancelled")).await }
            });
            tokio::time::timeout(Duration::from_secs(5), plugin.entered.notified()).await?;
            assert!(plugin.live.load(std::sync::atomic::Ordering::SeqCst));
            task.abort();
            assert!(task.await.is_err());
            #[cfg(feature = "washlet")]
            drop(lease);
            tokio::time::timeout(Duration::from_secs(1), plugin.cleanup_entered.notified()).await?;
            let refused = host
                .workload_start(empty_workload_start_request("cancelled"))
                .await?;
            assert_eq!(refused.workload_status.workload_state, WorkloadState::Error);
            #[cfg(feature = "washlet")]
            assert!(host.acquire_control().is_err());
            plugin.cleanup_gate.add_permits(1);
            tokio::time::timeout(Duration::from_secs(1), host.wait_for_workload_cleanup())
                .await??;
            assert!(!plugin.live.load(std::sync::atomic::Ordering::SeqCst));
            let reused = host
                .workload_start(empty_workload_start_request("cancelled"))
                .await?;
            assert_eq!(
                reused.workload_status.workload_state,
                WorkloadState::Running
            );
            assert_eq!(
                host.workload_status(WorkloadStatusRequest {
                    workload_id: "local".into()
                })
                .await?
                .workload_status
                .workload_state,
                WorkloadState::Running
            );
            #[cfg(feature = "washlet")]
            assert!(host.acquire_control().is_ok());
            host.stop().await?;
        }
        Ok(())
    }

    /// Nothing outside the host owes a cancelled start a second stop, so a
    /// cleanup that fails is the host's to retry. The id stays held meanwhile.
    #[tokio::test]
    async fn failed_cancelled_start_cleanup_is_retried_until_it_succeeds() -> anyhow::Result<()> {
        let plugin = Arc::new(CancelledBindingPlugin {
            cancel_at: CancelAt::ItemBind,
            entered: tokio::sync::Notify::new(),
            cleanup_entered: tokio::sync::Notify::new(),
            // One permit: the attempt that fails spends it, and the retry
            // waits here until the test lets it through.
            cleanup_gate: tokio::sync::Semaphore::new(1),
            live: std::sync::atomic::AtomicBool::new(false),
            fail_cleanup: std::sync::atomic::AtomicBool::new(true),
        });
        let host = Host::builder()
            .with_plugin(plugin.clone())?
            .build()?
            .start()
            .await?;
        let task = tokio::spawn({
            let host = host.clone();
            async move { host.workload_start(marker_request("retry")).await }
        });
        tokio::time::timeout(Duration::from_secs(5), plugin.entered.notified()).await?;
        task.abort();
        assert!(task.await.is_err());
        // The first attempt fails; the retry is waiting on the gate. Enabled
        // up front so neither notification is missed between the two waits.
        let mut retried = std::pin::pin!(plugin.cleanup_entered.notified());
        retried.as_mut().enable();
        tokio::time::timeout(Duration::from_secs(5), plugin.cleanup_entered.notified()).await?;
        tokio::time::timeout(Duration::from_secs(5), retried).await?;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), host.wait_for_workload_cleanup())
                .await
                .is_err()
        );
        assert!(plugin.live.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            host.workload_start(empty_workload_start_request("retry"))
                .await?
                .workload_status
                .workload_state,
            WorkloadState::Error
        );
        plugin.cleanup_gate.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), host.wait_for_workload_cleanup()).await??;
        assert!(!plugin.live.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            host.workload_start(empty_workload_start_request("retry"))
                .await?
                .workload_status
                .workload_state,
            WorkloadState::Running
        );
        host.stop().await
    }

    /// A stop whose teardown fails answers `Stopping`, not an error: the id is
    /// held and the host keeps retrying, and a caller that reads an error as
    /// the workload's final state would never ask again.
    #[tokio::test]
    async fn a_stop_whose_cleanup_fails_reports_stopping_and_is_retried() -> anyhow::Result<()> {
        let first = Arc::new(ResumableStartPlugin::new("first", 8));
        first
            .cleanup_failures
            .store(2, std::sync::atomic::Ordering::SeqCst);
        let last = Arc::new(ResumableStartPlugin::new("last", 1));
        let host = Host::builder()
            .with_plugin(first.clone())?
            .with_plugin(last.clone())?
            .build()?
            .start()
            .await?;
        host.workload_start(two_plugin_request("failing-stop")?)
            .await?;
        let stopped = host
            .workload_stop(WorkloadStopRequest {
                workload_id: "failing-stop".into(),
            })
            .await?
            .workload_status;
        assert_eq!(stopped.workload_state, WorkloadState::Stopping);
        assert!(stopped.message.contains("retried"), "{}", stopped.message);
        assert!(host.workload_reserve("failing-stop").await.is_err());
        // The stop's attempt and the first retry fail; the second retry succeeds.
        tokio::time::timeout(Duration::from_secs(5), host.wait_for_workload_cleanup()).await??;
        assert_eq!(
            first
                .cleanup_attempts
                .load(std::sync::atomic::Ordering::SeqCst),
            3
        );
        assert_eq!(
            last.cleanup_attempts
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            host.workload_status(WorkloadStatusRequest {
                workload_id: "failing-stop".into(),
            })
            .await?
            .workload_status
            .workload_state,
            WorkloadState::NotFound
        );
        host.stop().await
    }

    #[tokio::test]
    async fn a_plugin_failed_start_keeps_its_reservation_through_cancellation() -> anyhow::Result<()>
    {
        for stop_first in [false, true] {
            let plugin = Arc::new(CancelledBindingPlugin {
                cancel_at: CancelAt::ItemBind,
                entered: tokio::sync::Notify::new(),
                cleanup_entered: tokio::sync::Notify::new(),
                cleanup_gate: tokio::sync::Semaphore::new(0),
                live: std::sync::atomic::AtomicBool::new(false),
                fail_cleanup: std::sync::atomic::AtomicBool::new(false),
            });
            let host = Host::builder()
                .with_plugin(plugin.clone())?
                .build()?
                .start()
                .await?;
            let task = tokio::spawn({
                let host = host.clone();
                async move { host.workload_start(marker_request("failed-start")).await }
            });
            tokio::time::timeout(Duration::from_secs(5), plugin.entered.notified()).await?;
            host.fail_workload("failed-start", "plugin failed mid-bind".into())
                .await;
            let status = host
                .workload_status(WorkloadStatusRequest {
                    workload_id: "failed-start".into(),
                })
                .await?;
            assert_eq!(
                status.workload_status.workload_state,
                WorkloadState::Stopping
            );
            assert!(status.workload_status.message.is_empty());
            if stop_first {
                host.workload_stop(WorkloadStopRequest {
                    workload_id: "failed-start".into(),
                })
                .await?;
                assert!(host.workload_reserve("failed-start").await.is_err());
            }
            task.abort();
            assert!(task.await.is_err());
            tokio::time::timeout(Duration::from_secs(1), plugin.cleanup_entered.notified()).await?;
            assert!(host.workload_reserve("failed-start").await.is_err());
            plugin.cleanup_gate.add_permits(1);
            tokio::time::timeout(Duration::from_secs(1), host.wait_for_workload_cleanup())
                .await??;
            assert!(!plugin.live.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(
                host.workload_start(empty_workload_start_request("failed-start"))
                    .await?
                    .workload_status
                    .workload_state,
                WorkloadState::Running
            );
            host.stop().await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_plugin_failure_during_preparation_releases_only_its_original_reservation()
    -> anyhow::Result<()> {
        let host = Host::builder().build()?.start().await?;
        let reservation = host
            .workload_reserve("preparation")
            .await
            .map_err(anyhow::Error::msg)?;
        host.fail_workload("preparation", "plugin failed during preparation".into())
            .await;
        host.workload_release("preparation", reservation + 1).await;
        assert!(host.workload_reserve("preparation").await.is_err());
        host.workload_release("preparation", reservation).await;
        let status = host
            .workload_status(WorkloadStatusRequest {
                workload_id: "preparation".into(),
            })
            .await?;
        assert_eq!(status.workload_status.workload_state, WorkloadState::Error);
        assert_eq!(
            status.workload_status.message,
            "plugin failed during preparation"
        );
        host.workload_stop(WorkloadStopRequest {
            workload_id: "preparation".into(),
        })
        .await?;
        assert_eq!(
            host.workload_start(empty_workload_start_request("preparation"))
                .await?
                .workload_status
                .workload_state,
            WorkloadState::Running
        );
        host.stop().await
    }

    struct ResumableStartPlugin {
        id: &'static str,
        resolved_entered: tokio::sync::Notify,
        resolved_gate: tokio::sync::Semaphore,
        fail_resolution: bool,
        cleanup_entered: tokio::sync::Notify,
        cleanup_gate: tokio::sync::Semaphore,
        cleanup_failures: std::sync::atomic::AtomicUsize,
        cleanup_attempts: std::sync::atomic::AtomicUsize,
        cleanup_successes: std::sync::atomic::AtomicUsize,
    }

    impl ResumableStartPlugin {
        fn new(id: &'static str, cleanup_permits: usize) -> Self {
            Self {
                id,
                resolved_entered: tokio::sync::Notify::new(),
                resolved_gate: tokio::sync::Semaphore::new(1),
                fail_resolution: false,
                cleanup_entered: tokio::sync::Notify::new(),
                cleanup_gate: tokio::sync::Semaphore::new(cleanup_permits),
                cleanup_failures: std::sync::atomic::AtomicUsize::new(0),
                cleanup_attempts: std::sync::atomic::AtomicUsize::new(0),
                cleanup_successes: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl HostPlugin for ResumableStartPlugin {
        fn id(&self) -> &'static str {
            self.id
        }

        fn world(&self) -> WitWorld {
            WitWorld {
                imports: HashSet::from([WitInterface::from(format!(
                    "test:probe/{}@0.1.0",
                    self.id
                ))]),
                exports: HashSet::new(),
            }
        }

        async fn on_workload_item_bind<'a>(
            &self,
            item: &mut crate::engine::workload::WorkloadItem<'a>,
            _interfaces: crate::plugin::WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            item.linker()
                .instance(&format!("test:probe/{}@0.1.0", self.id))?;
            Ok(())
        }

        async fn on_workload_resolved(
            &self,
            _workload: &ResolvedWorkload,
            _component_id: &str,
        ) -> anyhow::Result<()> {
            self.resolved_entered.notify_one();
            let permit = self.resolved_gate.acquire().await?;
            permit.forget();
            anyhow::ensure!(!self.fail_resolution, "resolution failed");
            Ok(())
        }

        async fn on_workload_unbind(
            &self,
            _workload_id: &str,
            interfaces: crate::plugin::WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            assert!(interfaces.contains("test", "probe", &[self.id]));
            self.cleanup_attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.cleanup_entered.notify_one();
            let permit = self.cleanup_gate.acquire().await?;
            permit.forget();
            if self
                .cleanup_failures
                .try_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                anyhow::bail!("cleanup failed");
            }
            anyhow::ensure!(
                self.cleanup_successes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    == 0,
                "plugin was already unbound"
            );
            Ok(())
        }
    }

    fn two_plugin_request(id: &str) -> anyhow::Result<WorkloadStartRequest> {
        let mut request = marker_request(id);
        let component = request
            .workload
            .components
            .first_mut()
            .context("missing marker component")?;
        component.bytes = wat::parse_str(
            r#"(component
            (import "test:probe/first@0.1.0" (instance))
            (import "test:probe/last@0.1.0" (instance))
        )"#,
        )?
        .into();
        request.workload.host_interfaces = vec![
            WitInterface::from("test:probe/first@0.1.0"),
            WitInterface::from("test:probe/last@0.1.0"),
        ];
        Ok(request)
    }

    // Attached control cancels this same native stop future
    // when its shutdown drain expires, while leaving the host running.
    #[tokio::test]
    async fn cancelled_native_stop_can_be_retried() -> anyhow::Result<()> {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let plugin = Arc::new(BindRecordingPlugin {
            unbind_gate: Some(gate.clone()),
            ..Default::default()
        });
        let host = host_with(plugin.clone()).start().await?;
        let result = host
            .workload_start(marker_request("stop-cancelled"))
            .await?;
        assert_eq!(
            result.workload_status.workload_state,
            WorkloadState::Running
        );
        let task = tokio::spawn({
            let host = host.clone();
            async move {
                host.workload_stop(WorkloadStopRequest {
                    workload_id: "stop-cancelled".into(),
                })
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), plugin.unbind_entered.notified()).await?;
        task.abort();
        assert!(task.await.is_err());
        gate.add_permits(1);
        host.workload_stop(WorkloadStopRequest {
            workload_id: "stop-cancelled".into(),
        })
        .await?;
        host.wait_for_workload_cleanup().await?;
        let status = host
            .workload_status(WorkloadStatusRequest {
                workload_id: "stop-cancelled".into(),
            })
            .await?;
        assert_eq!(
            status.workload_status.workload_state,
            WorkloadState::NotFound,
            "retry reported success but an interrupted stop still owns the id"
        );
        assert_eq!(plugin.unbound(), vec!["stop-cancelled"]);
        host.stop().await
    }

    #[tokio::test]
    async fn cancelled_stop_retries_only_unfinished_callbacks() -> anyhow::Result<()> {
        let first = Arc::new(ResumableStartPlugin::new("first", 0));
        let last = Arc::new(ResumableStartPlugin::new("last", 1));
        let host = Host::builder()
            .with_plugin(first.clone())?
            .with_plugin(last.clone())?
            .build()?
            .start()
            .await?;
        host.workload_start(two_plugin_request("stop-retry")?)
            .await?;
        #[cfg(feature = "washlet")]
        let lease = host.acquire_control()?;
        let task = tokio::spawn({
            let host = host.clone();
            async move {
                host.workload_stop(WorkloadStopRequest {
                    workload_id: "stop-retry".into(),
                })
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), first.cleanup_entered.notified()).await?;
        assert_eq!(
            last.cleanup_successes
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        task.abort();
        assert!(task.await.is_err());
        #[cfg(feature = "washlet")]
        drop(lease);
        // Recovery fails once and its retry waits on the gate, so neither the
        // id nor the attachment lease can be reused until the remaining
        // callback finishes — by the retry, or by a stop that hurries it.
        first
            .cleanup_failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        first.cleanup_gate.add_permits(1);
        assert!(
            tokio::time::timeout(Duration::from_millis(500), host.wait_for_workload_cleanup())
                .await
                .is_err()
        );
        assert!(host.workload_reserve("stop-retry").await.is_err());
        #[cfg(feature = "washlet")]
        {
            let refused = host.acquire_control().err().context("lease was free")?;
            assert!(refused.to_string().contains("stop-retry"), "{refused:#}");
        }
        first.cleanup_gate.add_permits(1);
        host.workload_stop(WorkloadStopRequest {
            workload_id: "stop-retry".into(),
        })
        .await?;
        host.wait_for_workload_cleanup().await?;
        assert_eq!(
            first
                .cleanup_successes
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            last.cleanup_attempts
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        #[cfg(feature = "washlet")]
        assert!(host.acquire_control().is_ok());
        assert_eq!(
            host.workload_start(empty_workload_start_request("stop-retry"))
                .await?
                .workload_status
                .workload_state,
            WorkloadState::Running
        );
        host.stop().await
    }

    #[tokio::test]
    async fn interrupted_start_teardown_resumes_without_repeating_completed_unbinds()
    -> anyhow::Result<()> {
        // Exercise both a stopped start and a normal resolution failure's rollback.
        for fail_resolution in [false, true] {
            let first = Arc::new(ResumableStartPlugin::new("first", 0));
            let last = Arc::new(ResumableStartPlugin {
                resolved_gate: tokio::sync::Semaphore::new(0),
                fail_resolution,
                ..ResumableStartPlugin::new("last", 1)
            });
            let host = Host::builder()
                .with_plugin(first.clone())?
                .with_plugin(last.clone())?
                .build()?
                .start()
                .await?;
            let request = two_plugin_request("interrupted-teardown")?;
            let task = tokio::spawn({
                let host = host.clone();
                async move { host.workload_start(request).await }
            });
            tokio::time::timeout(Duration::from_secs(5), last.resolved_entered.notified()).await?;
            if !fail_resolution {
                host.workload_stop(WorkloadStopRequest {
                    workload_id: "interrupted-teardown".into(),
                })
                .await?;
            }
            last.resolved_gate.add_permits(1);
            tokio::time::timeout(Duration::from_secs(1), first.cleanup_entered.notified()).await?;
            assert_eq!(
                last.cleanup_successes
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            task.abort();
            assert!(task.await.is_err());
            assert!(host.workload_reserve("interrupted-teardown").await.is_err());
            first.cleanup_gate.add_permits(1);
            tokio::time::timeout(Duration::from_secs(1), host.wait_for_workload_cleanup())
                .await??;
            assert_eq!(
                first
                    .cleanup_successes
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert_eq!(
                last.cleanup_attempts
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert_eq!(
                host.workload_start(empty_workload_start_request("interrupted-teardown"))
                    .await?
                    .workload_status
                    .workload_state,
                WorkloadState::Running
            );
            host.stop().await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_stop_retries_only_unfinished_teardown_of_a_stopped_or_failed_start()
    -> anyhow::Result<()> {
        for fail_start in [false, true] {
            // Two permits for the two attempts that fail; the retry after them
            // waits on the gate until the stop below lets it through.
            let first = Arc::new(ResumableStartPlugin::new("first", 2));
            first
                .cleanup_failures
                .store(2, std::sync::atomic::Ordering::SeqCst);
            let last = Arc::new(ResumableStartPlugin {
                resolved_gate: tokio::sync::Semaphore::new(0),
                ..ResumableStartPlugin::new("last", 1)
            });
            let host = Host::builder()
                .with_plugin(first.clone())?
                .with_plugin(last.clone())?
                .build()?
                .start()
                .await?;
            let request = two_plugin_request("retry-teardown")?;
            let task = tokio::spawn({
                let host = host.clone();
                async move { host.workload_start(request).await }
            });
            tokio::time::timeout(Duration::from_secs(5), last.resolved_entered.notified()).await?;
            if fail_start {
                host.fail_workload("retry-teardown", "plugin failed during resolution".into())
                    .await;
            } else {
                host.workload_stop(WorkloadStopRequest {
                    workload_id: "retry-teardown".into(),
                })
                .await?;
            }
            last.resolved_gate.add_permits(1);
            assert_eq!(
                task.await??.workload_status.workload_state,
                if fail_start {
                    WorkloadState::Error
                } else {
                    WorkloadState::Stopping
                }
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(500), host.wait_for_workload_cleanup())
                    .await
                    .is_err()
            );
            assert!(host.workload_reserve("retry-teardown").await.is_err());
            first.cleanup_gate.add_permits(1);
            let stopped = host
                .workload_stop(WorkloadStopRequest {
                    workload_id: "retry-teardown".into(),
                })
                .await?
                .workload_status;
            assert_eq!(stopped.workload_state, WorkloadState::Stopping);
            tokio::time::timeout(Duration::from_secs(5), host.wait_for_workload_cleanup())
                .await??;
            assert_eq!(
                first
                    .cleanup_attempts
                    .load(std::sync::atomic::Ordering::SeqCst),
                3
            );
            assert_eq!(
                first
                    .cleanup_successes
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert_eq!(
                last.cleanup_attempts
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert_eq!(
                host.workload_start(empty_workload_start_request("retry-teardown"))
                    .await?
                    .workload_status
                    .workload_state,
                WorkloadState::Running
            );
            host.stop().await?;
        }
        Ok(())
    }

    /// A start that answered with a failure owes its caller that reason for as
    /// long as the id is held. When its own cleanup fails and the recovery it
    /// leaves behind finishes the job, the reason has to survive as the
    /// workload's `Error` — it is the only place a status can read it from.
    #[tokio::test]
    async fn a_failed_start_keeps_its_reason_when_cleanup_needs_a_retry() -> anyhow::Result<()> {
        let first = Arc::new(ResumableStartPlugin::new("first", 2));
        first
            .cleanup_failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let last = Arc::new(ResumableStartPlugin {
            resolved_gate: tokio::sync::Semaphore::new(0),
            ..ResumableStartPlugin::new("last", 1)
        });
        let host = Host::builder()
            .with_plugin(first.clone())?
            .with_plugin(last.clone())?
            .build()?
            .start()
            .await?;
        let request = two_plugin_request("retained-failure")?;
        let task = tokio::spawn({
            let host = host.clone();
            async move { host.workload_start(request).await }
        });
        tokio::time::timeout(Duration::from_secs(5), last.resolved_entered.notified()).await?;
        host.fail_workload("retained-failure", "plugin failed during resolution".into())
            .await;
        last.resolved_gate.add_permits(1);
        assert_eq!(
            task.await??.workload_status.workload_state,
            WorkloadState::Error
        );
        // The start's own cleanup failed once; the recovery it left retries it.
        tokio::time::timeout(Duration::from_secs(1), host.wait_for_workload_cleanup()).await??;
        assert_eq!(
            first
                .cleanup_attempts
                .load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        let status = host
            .workload_status(WorkloadStatusRequest {
                workload_id: "retained-failure".into(),
            })
            .await?;
        assert_eq!(status.workload_status.workload_state, WorkloadState::Error);
        assert_eq!(
            status.workload_status.message,
            "plugin failed during resolution"
        );
        host.workload_stop(WorkloadStopRequest {
            workload_id: "retained-failure".into(),
        })
        .await?;
        assert_eq!(
            host.workload_start(empty_workload_start_request("retained-failure"))
                .await?
                .workload_status
                .workload_state,
            WorkloadState::Running
        );
        host.stop().await
    }

    #[test]
    fn a_native_plugin_cannot_silently_ignore_egress_policy() {
        let declared = crate::plugin::PluginBindingSet::new("bind-recording").with_egress_policy(
            Arc::from(["example.com".parse().unwrap()]),
            Arc::from([]),
            Arc::from([]),
        );
        let err = Host::builder()
            .with_plugin(Arc::new(BindRecordingPlugin::default()))
            .unwrap()
            .with_plugin_bindings(crate::plugin::PluginBindings::new().with_plugin(declared))
            .build()
            .expect_err("an unenforced native policy must fail startup");
        assert!(format!("{err:#}").contains("cannot enforce"));
    }

    /// `build()` runs each plugin's own parser over the operator's
    /// declaration, so a binding written wrong fails startup rather than the
    /// first workload that asks for it. The message has to name the binding.
    ///
    /// Also the other direction: a named binding that omits `servers` inherits
    /// the host's data-plane address at resolve time, so it is complete by the
    /// time the plugin reads it and must *not* fail startup.
    #[cfg(feature = "wasmcloud-nats")]
    #[test]
    fn a_bad_declaration_fails_at_startup() {
        use crate::plugin::wasmcloud_nats::{PLUGIN_NATS_ID, WasmcloudNats};
        use crate::plugin::{PluginBindingSet, PluginBindings};

        let build = |binding: &[(&str, &str)]| {
            let declared = PluginBindingSet::new(PLUGIN_NATS_ID)
                .with_binding(
                    "orders",
                    binding
                        .iter()
                        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                        .collect(),
                )
                .with_default_bundle("servers", [("servers", "nats://data:4222")]);
            Host::builder()
                .with_plugin(Arc::new(WasmcloudNats::new()))
                .expect("failed to register plugin")
                .with_plugin_bindings(PluginBindings::new().with_plugin(declared))
                .build()
        };

        let err = build(&[("ack-mode", "sometimes")]).expect_err("not an ack mode");
        // Alternate form: the binding name is in the context chain, and a
        // message that stops at "invalid declaration" sends an operator
        // through every binding they have.
        let err = format!("{err:#}");
        assert!(err.contains("orders"), "names the binding: {err}");

        build(&[("subject-allow", "orders.>")])
            .expect("a binding that inherits the data-plane address is complete");
    }

    #[cfg(feature = "wasmcloud-nats")]
    #[test]
    fn nats_checks_declared_egress_before_connecting() {
        use crate::plugin::wasmcloud_nats::{PLUGIN_NATS_ID, WasmcloudNats};

        let build = |server: &str| {
            let declared = crate::plugin::PluginBindingSet::new(PLUGIN_NATS_ID)
                .with_binding(
                    "orders",
                    [("servers".to_string(), server.to_string())]
                        .into_iter()
                        .collect(),
                )
                .with_egress_policy(
                    Arc::from(["nats://data.example:4222".parse().unwrap()]),
                    Arc::from(["data.example".parse().unwrap()]),
                    Arc::from([]),
                );
            Host::builder()
                .with_plugin(Arc::new(WasmcloudNats::new()))
                .unwrap()
                .with_plugin_bindings(crate::plugin::PluginBindings::new().with_plugin(declared))
                .build()
        };

        build("nats://data.example:4222").expect("declared server is allowed");
        let err =
            build("nats://other.example:4222").expect_err("an undeclared server must fail startup");
        assert!(format!("{err:#}").contains("allowedIpNameLookups"));
    }

    /// A start that fails *after* its plugins bound must give the binding back.
    /// `resolve` rolls back its own failures; anything failing later has to be
    /// released by the start itself, or every plugin is left holding
    /// per-workload state for a workload that never ran — and, for a plugin
    /// that can call into workloads, still able to reach it.
    #[tokio::test]
    async fn test_start_failing_after_bind_unbinds_plugins() {
        let plugin = Arc::new(BindRecordingPlugin::default());
        let host = host_with(Arc::clone(&plugin));

        let response = host
            .workload_start(service_fails_after_bind_request("late-failure"))
            .await
            .expect("workload_start should report rather than error");

        assert_eq!(
            response.workload_status.workload_state,
            WorkloadState::Error,
            "the service cannot start, so the workload is in error"
        );
        assert_eq!(
            plugin.bound(),
            vec!["late-failure".to_string()],
            "the plugin should have been bound before the failure; start said: {}",
            response.workload_status.message
        );
        assert_eq!(
            plugin.unbound(),
            vec!["late-failure".to_string()],
            "a start that fails after binding must unbind"
        );
    }

    /// Stopping a workload that failed to start is a no-op beyond dropping the
    /// id: the failure path already released it. In particular the plugin must
    /// not be unbound a second time.
    #[tokio::test]
    async fn test_stopping_an_errored_workload_does_not_unbind_twice() {
        let plugin = Arc::new(BindRecordingPlugin::default());
        let host = host_with(Arc::clone(&plugin));

        host.workload_start(service_fails_after_bind_request("errored"))
            .await
            .expect("workload_start should report rather than error");
        host.workload_stop(WorkloadStopRequest {
            workload_id: "errored".to_string(),
        })
        .await
        .expect("stopping an errored workload should succeed");

        assert_eq!(
            plugin.unbound(),
            vec!["errored".to_string()],
            "the errored workload was already released; stop must not unbind again"
        );
        assert!(
            host.workloads.read().await.is_empty(),
            "stopping should drop the id"
        );
    }

    /// A stopped host takes no new workloads. Its ingress has stopped
    /// accepting and its plugins are stopping, so one started onto it would be
    /// unroutable — and would be unregistered without a word when the ingress
    /// finishes draining.
    #[tokio::test]
    async fn test_a_stopped_host_refuses_to_start_a_workload() {
        let host = Arc::new(host_with(Arc::new(BindRecordingPlugin::default())));
        Arc::clone(&host)
            .stop()
            .await
            .expect("stopping the host should succeed");

        let refused = host
            .workload_start(marker_request("after-stop"))
            .await
            .expect("a refusal is a response, not a transport failure");
        assert_eq!(
            refused.workload_status.workload_state,
            WorkloadState::Error,
            "a stopped host must refuse a start, got {:?}",
            refused.workload_status
        );
        assert!(
            refused.workload_status.message.contains("stopped"),
            "the refusal must say why, got {:?}",
            refused.workload_status.message
        );
        assert!(
            !host.workloads.read().await.contains_key("after-stop"),
            "a refused start must leave no reservation behind"
        );
    }

    /// A stop that arrives while a workload is still starting cannot tear it
    /// down — there is nothing built yet — so it leaves a `Stopping` marker and
    /// the start owns the cleanup: it finds its slot taken, releases what it
    /// built, and drops the id itself. Without that the workload's plugins stay
    /// bound and its service keeps running with no record of either.
    #[tokio::test]
    async fn test_stop_racing_a_start_releases_the_workload() {
        let plugin = Arc::new(BindRecordingPlugin {
            bind_delay: Duration::from_millis(300),
            ..Default::default()
        });
        let host = Arc::new(host_with(Arc::clone(&plugin)));

        let starting = {
            let host = Arc::clone(&host);
            tokio::spawn(async move { host.workload_start(marker_request("raced")).await })
        };

        // Stop while the plugin's bind is still sleeping.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let stopped = host
            .workload_stop(WorkloadStopRequest {
                workload_id: "raced".to_string(),
            })
            .await
            .expect("stopping a starting workload should succeed");
        assert_eq!(
            stopped.workload_status.workload_state,
            WorkloadState::Stopping
        );

        let started = starting
            .await
            .expect("the start task should not panic")
            .expect("workload_start should report rather than error");
        assert_eq!(
            started.workload_status.workload_state,
            WorkloadState::Stopping,
            "a start whose slot was taken by a stop reports that, not Running"
        );

        assert_eq!(
            plugin.unbound(),
            vec!["raced".to_string()],
            "the start must release what it bound once it finds its slot stopped"
        );
        assert!(
            host.workloads.read().await.is_empty(),
            "the id is dropped only once the start has released the workload"
        );
    }

    /// A teardown holds its workload id for as long as it runs, so a second
    /// stop arriving mid-teardown cannot free the id. Were it able to, a start
    /// could claim the id while the first teardown is still unbinding — and
    /// `unbind_all_plugins` is keyed by workload id, so the newcomer would be
    /// unbound by its predecessor's teardown and then dropped from the map
    /// entirely, left running with nothing tracking it.
    #[tokio::test]
    async fn test_a_second_stop_does_not_free_an_in_flight_teardown() {
        let plugin = Arc::new(BindRecordingPlugin {
            unbind_delay: Duration::from_millis(300),
            ..Default::default()
        });
        let host = Arc::new(host_with(Arc::clone(&plugin)));
        host.workload_start(marker_request("held"))
            .await
            .expect("workload_start should report rather than error");

        let stopping = {
            let host = Arc::clone(&host);
            tokio::spawn(async move {
                host.workload_stop(WorkloadStopRequest {
                    workload_id: "held".to_string(),
                })
                .await
            })
        };

        // A retried stop, while the first one's unbind is still sleeping.
        tokio::time::sleep(Duration::from_millis(50)).await;
        host.workload_stop(WorkloadStopRequest {
            workload_id: "held".to_string(),
        })
        .await
        .expect("a second stop should report rather than error");
        assert!(
            host.workloads.read().await.contains_key("held"),
            "the id stays reserved while the first stop is still tearing down"
        );

        // ...so a redeploy under the same id cannot slip in behind it.
        let redeployed = host
            .workload_start(marker_request("held"))
            .await
            .expect("workload_start should report rather than error");
        assert_eq!(
            redeployed.workload_status.workload_state,
            WorkloadState::Error,
            "the id is still taken, so a start under it is refused rather than racing the teardown"
        );

        stopping
            .await
            .expect("the stop task should not panic")
            .expect("stopping should succeed");
        assert_eq!(
            plugin.unbound(),
            vec!["held".to_string()],
            "the workload is unbound exactly once, by the stop that owned it"
        );
        assert!(
            host.workloads.read().await.is_empty(),
            "the id is dropped once its teardown finishes"
        );
    }

    /// A plugin failing a workload that is still starting hands the teardown
    /// to the start, which publishes the failure as `Error` only once it has
    /// released what it bound.
    #[tokio::test]
    async fn test_a_failure_mid_start_is_published_once_released() {
        let plugin = Arc::new(BindRecordingPlugin {
            bind_delay: Duration::from_millis(300),
            ..Default::default()
        });
        let host = Arc::new(host_with(Arc::clone(&plugin)));

        let starting = {
            let host = Arc::clone(&host);
            tokio::spawn(async move { host.workload_start(marker_request("evicted")).await })
        };

        // Fail it while the plugin's bind is still sleeping.
        tokio::time::sleep(Duration::from_millis(50)).await;
        host.fail_workload("evicted", "crash-looping".to_string())
            .await;

        let started = starting
            .await
            .expect("the start task should not panic")
            .expect("workload_start should report rather than error");
        assert_eq!(started.workload_status.workload_state, WorkloadState::Error);
        assert_eq!(started.workload_status.message, "crash-looping");
        assert_eq!(
            plugin.unbound(),
            vec!["evicted".to_string()],
            "the start releases what it bound, once"
        );
        assert!(
            matches!(
                host.workloads.read().await.get("evicted"),
                Some(HostWorkload::Error(reason)) if reason == "crash-looping"
            ),
            "the failure is published once the teardown is done"
        );
    }

    /// A stop arriving after a mid-start failure takes effect: the id stays
    /// reserved while the start tears down — a redeploy claiming it then would
    /// be unbound by that teardown, which is keyed by workload id — and is
    /// dropped once it is done, rather than left behind as `Error` for a
    /// second stop to collect.
    #[tokio::test]
    async fn test_a_stop_after_a_mid_start_failure_frees_the_id_once_released() {
        let plugin = Arc::new(BindRecordingPlugin {
            bind_delay: Duration::from_millis(300),
            unbind_delay: Duration::from_millis(300),
            ..Default::default()
        });
        let host = Arc::new(host_with(Arc::clone(&plugin)));

        let starting = {
            let host = Arc::clone(&host);
            tokio::spawn(async move { host.workload_start(marker_request("evicted")).await })
        };

        tokio::time::sleep(Duration::from_millis(50)).await;
        host.fail_workload("evicted", "crash-looping".to_string())
            .await;
        host.workload_stop(WorkloadStopRequest {
            workload_id: "evicted".to_string(),
        })
        .await
        .expect("a stop should report rather than error");
        let redeployed = host
            .workload_start(marker_request("evicted"))
            .await
            .expect("workload_start should report rather than error");
        assert_eq!(
            redeployed.workload_status.workload_state,
            WorkloadState::Error,
            "the id is still held, so a start under it is refused"
        );

        starting
            .await
            .expect("the start task should not panic")
            .expect("workload_start should report rather than error");
        assert_eq!(
            plugin.unbound(),
            vec!["evicted".to_string()],
            "the start releases what it bound, once"
        );
        assert!(
            host.workloads.read().await.is_empty(),
            "the stop takes effect once the teardown is done"
        );
    }

    /// Records the workloads it is told to unbind.
    #[derive(Default)]
    struct UnbindRecordingHandler {
        unbound: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl crate::host::http::HostHandler for UnbindRecordingHandler {
        async fn start(&self) -> anyhow::Result<()> {
            Ok(())
        }

        async fn stop(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn port(&self) -> u16 {
            0
        }

        async fn on_workload_resolved(
            &self,
            _resolved_handle: &ResolvedWorkload,
            _component_id: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
            self.unbound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(workload_id.to_string());
            Ok(())
        }

        fn outgoing_request(
            &self,
            _workload_id: &str,
            _request: hyper::Request<wasmtime_wasi_http::WasiBody>,
            _options: Option<wasmtime_wasi_http::RequestOptions>,
            _fut: crate::host::http::RequestIoFuture,
            _allowed_hosts: &[crate::host::allowed_hosts::AllowedHost],
        ) -> crate::host::http::SendFuture {
            Box::new(async {
                Err(wasmtime_wasi_http::Error::InternalError(Some(
                    "no egress in this test".to_string(),
                )))
            })
        }
    }

    /// A workload that serves no HTTP can still send it, and its egress state
    /// lives in the HTTP handler, so stopping it must reach the handler too.
    #[tokio::test]
    async fn test_stopping_a_workload_that_serves_no_http_unbinds_the_http_handler() {
        let handler = Arc::new(UnbindRecordingHandler::default());
        let host = Host::builder()
            .with_plugin(Arc::new(BindRecordingPlugin::default()))
            .expect("failed to register plugin")
            .with_http_handler(Arc::clone(&handler) as Arc<dyn crate::host::http::HostHandler>)
            .build()
            .expect("failed to build host");
        let started = host
            .workload_start(marker_request("sender"))
            .await
            .expect("workload_start should report rather than error");
        assert_eq!(
            started.workload_status.workload_state,
            WorkloadState::Running
        );

        host.workload_stop(WorkloadStopRequest {
            workload_id: "sender".to_string(),
        })
        .await
        .expect("stopping should succeed");
        assert_eq!(*handler.unbound.lock().unwrap(), ["sender"]);
    }

    #[test]
    fn test_extract_component_interfaces_with_http_export() {
        // Create a component that exports wasi:http/incoming-handler
        // Using import syntax since WAT exports require actual implementations
        let wat = r#"
            (component
                (import "wasi:http/incoming-handler@0.2.0" (instance))
            )
        "#;
        let component_bytes = wat::parse_str(wat).expect("failed to parse WAT");

        let host = Host::builder().build().expect("failed to build host");

        let interfaces = host
            .intersect_interfaces(&component_bytes)
            .expect("failed to extract interfaces");

        // Should have extracted 1 interface
        assert_eq!(interfaces.len(), 1, "expected 1 interface");

        // Check for wasi:http interface
        let http_interface = interfaces
            .iter()
            .find(|i| i.namespace == "wasi" && i.package == "http")
            .expect("wasi:http interface not found");
        assert!(
            http_interface.interfaces.contains("incoming-handler"),
            "should contain incoming-handler interface"
        );
    }

    #[test]
    fn test_extract_component_interfaces_no_interfaces() {
        // Component with no imports or exports
        let wat = r#"
            (component)
        "#;
        let component_bytes = wat::parse_str(wat).expect("failed to parse WAT");

        let host = Host::builder().build().expect("failed to build host");

        let interfaces = host
            .intersect_interfaces(&component_bytes)
            .expect("failed to extract interfaces");

        assert_eq!(
            interfaces.len(),
            0,
            "expected no interfaces for component with no imports/exports"
        );
    }

    #[test]
    fn test_extract_component_interfaces_invalid_bytes() {
        let invalid_bytes = b"not a valid component";

        let host = Host::builder().build().expect("failed to build host");

        let result = host.intersect_interfaces(invalid_bytes);
        assert!(
            result.is_err(),
            "should fail to extract interfaces from invalid bytes"
        );
    }
}
