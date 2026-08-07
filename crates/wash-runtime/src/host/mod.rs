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
use crate::observability::Meters;
use crate::plugin::{HostPlugin, WorkloadFailure, WorkloadFailureSink};
use crate::types::*;
use crate::wit::{WitInterface, WitWorld};

pub mod quota;
mod sysinfo;
use sysinfo::SystemMonitor;

pub mod allowed_hosts;
pub mod allowed_ip_name;
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
    Error(String),
}

/// Give back everything binding a workload allocated: its service, then its
/// plugins.
///
/// Every path that lets go of a bound workload goes through here — a start that
/// failed after binding, a start that lost its slot to a concurrent stop, a
/// plugin failing a running workload, and an ordinary stop. Tolerates a
/// workload that never fully started: both steps have nothing to do when there
/// is nothing to undo.
///
/// Which slot in the workload map may be written, and by whom, is what keeps
/// this safe to run outside the map's lock: `unbind_all_plugins` is keyed by
/// workload id, so the id must stay reserved until this returns or a new
/// workload could claim it and be torn down by someone else's teardown. Every
/// caller therefore leaves a [`HostWorkload::Stopping`] carrying its own
/// [`Reservation`] over the whole call. See [`HostApi::workload_stop`] for the
/// ownership rules that guarantee it.
async fn release(workload_id: &str, resolved: &ResolvedWorkload) {
    resolved.stop_service();
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
            HostWorkload::Error(err) => write!(f, "Error: {err}"),
        }
    }
}

impl From<&HostWorkload> for WorkloadState {
    fn from(hw: &HostWorkload) -> Self {
        match hw {
            HostWorkload::Starting(_) => WorkloadState::Starting,
            HostWorkload::Running(_) => WorkloadState::Running,
            HostWorkload::Stopping(_) => WorkloadState::Stopping,
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
    /// Source of the [`Reservation`] a start or a teardown stamps on the slot it
    /// owns. Monotonic for the life of the host, so a reservation identifies one
    /// occupant of a workload id and never a later one.
    reservations: std::sync::atomic::AtomicU64,
    /// Plugins in a map from their ID to the plugin itself
    plugins: HashMap<&'static str, Arc<dyn HostPlugin>>,
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
    config: HostConfig,
    meters: Meters,
}

impl Host {
    /// Create a new builder for the host.
    pub fn builder() -> HostBuilder {
        HostBuilder::default()
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
    /// `Stopping(reservation)`: drop the id, or leave `Some(reason)` behind as
    /// the workload's `Error` for a later stop to collect.
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
        if !matches!(workloads.get(workload_id), Some(HostWorkload::Stopping(held)) if *held == reservation)
        {
            return;
        }
        match reason {
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
                // owns everything it has built. Recording the failure now is
                // what tells it its slot is gone, so it releases what it bound
                // and leaves this `Error` for a stop to collect.
                Some(slot @ HostWorkload::Starting(_)) => {
                    *slot = HostWorkload::Error(reason.clone());
                    None
                }
                // Already being torn down, already failed, or gone: the workload
                // is on its way out either way, and the slot belongs to whoever
                // is finishing it.
                Some(HostWorkload::Stopping(_) | HostWorkload::Error(_)) | None => None,
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
        self.http_handler
            .stop()
            .await
            .context("failed to stop HTTP handler")?;

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
    ) -> anyhow::Result<ResolvedWorkload> {
        let service_present = request.workload.service.is_some();
        let workload_id = request.workload_id.clone();

        // Initialize the workload using the engine, receiving the unresolved workload
        let unresolved_workload = self
            .engine
            .initialize_workload(&request.workload_id, request.workload)?;

        // `resolve` binds the workload's plugins, and gives back whatever it
        // bound if any part of that fails.
        let mut resolved_workload = unresolved_workload
            .resolve(Some(&self.plugins), self.http_handler.clone())
            .await?;

        // Past this point the plugins are bound, so every exit either hands the
        // workload to the caller to commit or gives the binding back. The rest
        // of starting lives in one function rather than inline, so a step added
        // to it is covered by this rollback instead of needing its own.
        if let Err(e) = start_resolved(&workload_id, &mut resolved_workload, service_present).await
        {
            release(&workload_id, &resolved_workload).await;
            return Err(e);
        }

        Ok(resolved_workload)
    }
}

/// Everything after plugin binding that can still fail while starting a
/// workload. Kept together so [`Host::workload_start_inner`] has exactly one
/// failure path to roll back.
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
        // Reserve the workload ID while holding the write lock so concurrent
        // starts cannot both observe it as available. An ID remains reserved
        // in every lifecycle state until workload_stop removes it. The
        // reservation stamped on the slot is what lets the commit below tell
        // this start's slot from one a later start claimed.
        let reservation = self.reserve();
        {
            let mut workloads = self.workloads.write().await;
            if workloads.contains_key(&request.workload_id) {
                return Ok(WorkloadStartResponse {
                    workload_status: WorkloadStatus {
                        workload_id: request.workload_id.clone(),
                        workload_state: WorkloadState::Error,
                        message: format!(
                            "Workload ID [{}] already exists (the exising workload must be stopped to reuse the ID)",
                            request.workload_id
                        ),
                    },
                });
            }
            workloads.insert(
                request.workload_id.clone(),
                HostWorkload::Starting(reservation),
            );
        }

        let workload_id = request.workload_id.clone();
        let started = self.workload_start_inner(request).await;

        // Commit under the same lock the id was reserved under, and only into
        // the slot this start reserved. Anything else in that slot means the
        // workload was stopped or failed while this was still starting: writing
        // `Running` over it would resurrect a workload nobody is tracking, and
        // simply dropping the result — which is what an `and_modify` that finds
        // no entry does — would leave its plugins bound and its service running
        // detached.
        let (workload_state, message, orphaned) = {
            let mut workloads = self.workloads.write().await;
            let slot = workloads.get(&workload_id);
            let mine = matches!(slot, Some(HostWorkload::Starting(held)) if *held == reservation);
            // A stop that arrived mid-start handed the teardown back by marking
            // the slot `Stopping` under this start's own reservation.
            let handed_back =
                matches!(slot, Some(HostWorkload::Stopping(held)) if *held == reservation);
            match started {
                Ok(resolved) if mine => {
                    workloads.insert(
                        workload_id.clone(),
                        HostWorkload::Running(Box::new(resolved)),
                    );
                    (
                        WorkloadState::Running,
                        "Workload started successfully".to_string(),
                        None,
                    )
                }
                // Stopped (or failed) while starting. The stop could not tear
                // this down because it did not exist yet, so that falls to us.
                // The `Stopping` marker is left in place for the whole teardown:
                // it keeps the id reserved, so a new start cannot claim it and
                // then be unbound by our teardown.
                Ok(resolved) => (
                    WorkloadState::Stopping,
                    "Workload was stopped while starting".to_string(),
                    Some(resolved),
                ),
                Err(err) => {
                    let message = err.to_string();
                    if mine {
                        workloads.insert(workload_id.clone(), HostWorkload::Error(message.clone()));
                    } else if handed_back {
                        // A stop is waiting on this start to finish. Nothing is
                        // bound — `workload_start_inner` released it before
                        // returning — so the id can go now.
                        workloads.remove(&workload_id);
                    }
                    (WorkloadState::Error, message, None)
                }
            }
        };

        if let Some(resolved) = orphaned {
            release(&workload_id, &resolved).await;
            // Only if the `Stopping` marker is still this start's. It may not be
            // — a failure reported mid-start writes `Error` over it — and then
            // the slot is not ours to drop.
            self.finish_teardown(&workload_id, reservation, None).await;
        }

        Ok(WorkloadStartResponse {
            workload_status: WorkloadStatus {
                workload_id,
                workload_state,
                message,
            },
        })
    }

    #[instrument(skip_all, fields(workload.id = request.workload_id))]
    async fn workload_status(
        &self,
        request: WorkloadStatusRequest,
    ) -> anyhow::Result<WorkloadStatusResponse> {
        if let Some(workload) = self.workloads.read().await.get(&request.workload_id) {
            let workload_state = workload.into();
            Ok(WorkloadStatusResponse {
                workload_status: WorkloadStatus {
                    workload_id: request.workload_id,
                    message: format!("Workload is {workload}"),
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
            // - `Starting`: the start owns it and has not produced a workload
            //   yet. Leave a `Stopping` marker carrying the start's own
            //   reservation; the start sees it, tears down what it built, and
            //   drops the id.
            // - `Stopping`: a teardown is already under way and the id stays
            //   reserved until it finishes. Repeating the stop cannot help and
            //   freeing the id would hand it to a new workload that the running
            //   teardown would then unbind, so this stop reports the state and
            //   leaves the slot alone.
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
                    Some(HostWorkload::Starting(held)) => StopAction::Mark(*held),
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
                debug!(
                    workload_id = request.workload_id,
                    workload_name = resolved_workload.name(),
                    "stopping workload"
                );
                release(&request.workload_id, &resolved_workload).await;
                // Dropped only now, so the id stays reserved for the whole
                // teardown.
                self.finish_teardown(&request.workload_id, reservation, None)
                    .await;
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
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            allow_oci_insecure: false,
            oci_pull_timeout: Duration::from_secs(30).into(),
            oci_cache_dir: None,
        }
    }
}

/// Builder for the [`Host`]
pub struct HostBuilder {
    id: String,
    engine: Option<Engine>,
    plugins: HashMap<&'static str, Arc<dyn HostPlugin>>,
    hostname: Option<String>,
    friendly_name: Option<String>,
    environment: Option<String>,
    labels: HashMap<String, String>,
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
    config: Option<HostConfig>,
    meters: Meters,
}

impl Default for HostBuilder {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            engine: Default::default(),
            plugins: Default::default(),
            hostname: Default::default(),
            friendly_name: Default::default(),
            environment: Default::default(),
            labels: Default::default(),
            http_handler: Default::default(),
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
        self.meters = meters;
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
    /// Returns an error if the default engine cannot be created (when no engine is provided).
    pub fn build(self) -> anyhow::Result<Host> {
        let engine = if let Some(engine) = self.engine {
            engine
        } else {
            Engine::builder().build()?
        };

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

        Ok(Host {
            engine,
            workloads: Arc::default(),
            reservations: std::sync::atomic::AtomicU64::default(),
            plugins: self.plugins,
            id: self.id,
            hostname,
            friendly_name,
            environment: self.environment.unwrap_or_default(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            labels: self.labels,
            started_at: chrono::Utc::now(),
            system_monitor: Arc::new(RwLock::new(SystemMonitor::new())),
            http_handler,
            config: self.config.unwrap_or_default(),
            meters: self.meters,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Component;

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
