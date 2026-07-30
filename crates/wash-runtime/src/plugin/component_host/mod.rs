//! Host component plugins: a host capability provided by a WebAssembly
//! *component* rather than by Rust code.
//!
//! Where a Rust [`HostPlugin`] installs its implementation into the caller's own
//! store via `add_to_linker` (running as `ActiveCtx` host-trait methods), a host
//! component plugin runs in **its own long-lived, supervised store** and is
//! reached across the store boundary. It is the Service co-driver pattern
//! generalized to host scope: one persistent, `run_concurrent`-driven store
//! (the [`crate::host::trigger_service::TriggerService`] with a [`Ingress::Capability`]),
//! instantiated once at host start, serving concurrent capability calls from
//! every workload that imports the interface it exports.
//!
//! [`ComponentHostPlugin`] is the adapter that flows a wasm plugin through the
//! unchanged `bind_plugins` matching machinery:
//! - [`ComponentHostPlugin::world`] is derived from the component's exported
//!   interfaces, so `includes_bidirectional` matches a workload's import.
//! - [`ComponentHostPlugin::start`] instantiates the persistent store + driver
//!   under supervision.
//! - [`ComponentHostPlugin::on_workload_item_bind`] installs `func_new_concurrent`
//!   shims on the workload's linker that route each call to the persistent store
//!   — instead of `add_to_linker`.
//!
//! Arguments and results cross the boundary via [`crate::engine::store::relocate`]:
//! handle-free values are copied; `stream<T>`/`future<T>` handles are relocated
//! (pumped); and `resource` handles are proxied — `own<r>` returns become a
//! proxy in the caller, `borrow<r>`/method calls route to the real resource in
//! the plugin store, and dropping the proxy frees it (see
//! [`crate::engine::store::resource_bridge`]). A plugin may also import an interface it
//! exports (a self-import), wired back to the plugin itself; runaway re-entrant
//! recursion is bounded by the TriggerService's in-flight-task ceiling. A plugin that
//! imports `wasmcloud:host/identity` can partition state by its caller's
//! `(workload_id, component_id)`, attributed exactly under concurrency via the
//! caller's root guest task (tracked in the per-incarnation
//! [`crate::host::job_registry::JobRegistry`]). A plugin that imports
//! `wasmcloud:host/cancel` can cooperatively cancel one of its own in-flight
//! invocations: `request-cancel` marks the job and the guest unwinds itself
//! (polling `is-cancelled`, or observing a dropped stream reader) — without
//! disturbing the store's other tenants. Only `error-context` values remain
//! unsupported.
//!
//! A plugin may additionally *export* `wasmcloud:host/workload-lifecycle` to
//! hear about the workloads calling it, so it can provision and reclaim
//! per-workload state eagerly rather than lazily on first call. `wasmcloud:host`
//! exports are reserved: they are host-invoked contracts, never
//! workload-matchable capabilities. The whole hook path — signature check,
//! delivery, post-restart replay, and quarantine — lives in the `lifecycle`
//! submodule; this module drives it from `ComponentHostPlugin`'s `HostPlugin`
//! impl.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use tracing::{debug, error, warn};
use wasmtime::AsContextMut;
use wasmtime::component::types::Type;
use wasmtime::component::{
    Accessor, Component, GuestTaskId, InstancePre, Linker, Resource, Val, types::ComponentItem,
};
use wasmtime::{Store, StoreContextMut};

use crate::engine::Engine;
use crate::engine::ctx::{CallerIdentity, Ctx, SharedCtx};
use crate::engine::instance_pool::InstancePolicy;
use crate::engine::store::relocate::{self, Relocated};
use crate::engine::store::resource_bridge::{self, ProxyResource};
use crate::engine::workload::{UnresolvedWorkload, WorkloadComponent, WorkloadItem};
use crate::host::job_registry::JobRegistry;
use crate::host::trigger_service::{
    CapabilityCall, CapabilityFunc, CapabilityJob, Ingress, LifecycleReplay, TriggerService,
    decode_bind_reply,
};
use crate::oci::OciConfig;
use crate::plugin::component_plugin_spec::ComponentPluginSpec;
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::sockets::loopback;
use crate::types::LocalResources;
use crate::wit::{WitInterface, WitWorld};

mod lifecycle;

use lifecycle::{
    BindReply, HOST_LIFECYCLE_EXPORT, LifecycleFuncs, POISON_EVICT_STRIKES, attribute_replay_fault,
    await_bind_reply, evict_workload, lifecycle_funcs, remove_and_unbind, replay_snapshot,
    report_workload_failed, send_lifecycle_job, spawn_deferred_unbind, workload_info_val,
};

/// Capacity of a plugin incarnation's capability-call channel. Bounds queued
/// (not-yet-served) calls; in-flight (being-served) calls are separately capped
/// by the TriggerService's per-store in-flight-task ceiling.
const CAPABILITY_CHANNEL_CAPACITY: usize = 256;

/// Default number of times a plugin's driver is restarted under supervision
/// before the plugin is declared dead. One store now serves every workload, so
/// a restart story is required rather than optional.
const DEFAULT_MAX_RESTARTS: u32 = 3;

type CapabilitySender = tokio::sync::mpsc::Sender<CapabilityJob>;

/// One exported capability function, introspected from the plugin component's
/// type at construction. The param/result types drive the relocation pass that
/// moves arguments and results across the store boundary.
struct ExportedFunc {
    name: Arc<str>,
    param_tys: Arc<[Type]>,
    result_tys: Arc<[Type]>,
}

/// One exported capability interface the plugin provides.
struct ExportedInterface {
    /// Full component instance name, e.g. `acme:kv/store@0.1.0` — the exact
    /// string used to address the interface on both the plugin's own instance
    /// and a workload's linker.
    name: Arc<str>,
    /// Parsed form for `world()` derivation and plugin matching.
    wit: WitInterface,
    funcs: Vec<ExportedFunc>,
    /// Resource types the interface defines (e.g. `bucket`). Registered on a
    /// caller's linker as cross-store proxies; their methods/constructors/statics
    /// appear in `funcs` (they are ordinary interface functions).
    resources: Vec<Arc<str>>,
}

/// Whether an interface belongs to the reserved `wasmcloud:host` package —
/// host-invoked contracts (like the lifecycle export), never capabilities a
/// workload may import.
fn is_reserved(wit: &WitInterface) -> bool {
    wit.namespace == "wasmcloud" && wit.package == "host"
}

/// Runtime state shared between the plugin, the linker shims it installs, and
/// its supervisor task.
struct ComponentHostPluginState {
    id: &'static str,
    /// Sender for the *current* incarnation's capability channel. Swapped by the
    /// supervisor on a restart (so already-installed shims reach the new
    /// instance), and cleared on `stop()` (so the driver's serve loop ends). An
    /// `ArcSwapOption` so the per-call read in [`Self::sender`] is lock-free —
    /// writes happen only on start/stop/restart.
    tx: ArcSwapOption<CapabilitySender>,
    /// The supervisor task, taken and awaited on `stop()`.
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The *current* incarnation's job registry, swapped alongside `tx` on each
    /// (re)start. The host `identity`/`cancel` imports — baked into the reused
    /// linker at construction — read it from here so they reach the live store's
    /// registry. Lock-free reads for the same reason as `tx`.
    registry: ArcSwapOption<JobRegistry>,
    /// Workloads currently bound to this plugin, as workload id → the
    /// `workload-info` value delivered to `on-workload-bind`. The supervisor
    /// snapshots it on each (re)start to replay binds into the fresh
    /// incarnation. Empty unless the plugin exports the lifecycle interface.
    bound: Mutex<BTreeMap<Arc<str>, Val>>,
    /// Consecutive `on-workload-bind` traps attributed to each workload since it
    /// last bound cleanly — the quarantine strike count. Harvested from the
    /// faulted incarnation's registry after each fault; a workload reaching
    /// [`POISON_EVICT_STRIKES`] is evicted so its poison bind stops crash-looping
    /// the shared store for every other tenant. Reset for all workloads after a
    /// healthy uptime, like the restart budget.
    poison: Mutex<BTreeMap<Arc<str>, u32>>,
    /// Append-only log of workload ids whose `on-workload-bind` was observed to
    /// trap, in harvest order. Diagnostic, and the observable the attribution
    /// tests assert against.
    bind_trap_log: Mutex<Vec<Arc<str>>>,
    /// Sink for reporting an evicted workload to the host so its scheduling
    /// health becomes failed. Injected once at start; absent when the plugin is
    /// used without a host (e.g. driven directly in tests).
    failure_sink: ArcSwapOption<crate::plugin::WorkloadFailureSink>,
    /// Per-call budget for a lifecycle (bind/unbind) delivery, in milliseconds.
    /// Defaults to [`crate::timeouts::plugin_lifecycle_call`]; overridable via
    /// [`ComponentHostPlugin::with_lifecycle_call_timeout`]. Read on every
    /// lifecycle call, written at most once (before start), so a relaxed atomic
    /// is enough.
    lifecycle_timeout_ms: AtomicU64,
    /// The native (non-component) plugins this plugin's own imports resolved
    /// against at construction ([`link_native_imports`]). Reinjected into every
    /// incarnation's own `Ctx` ([`build_plugin_store`]) so a native's host
    /// function — reading its own plugin state via
    /// [`crate::engine::ctx::Ctx::try_get_plugin`] — finds it there the same
    /// way it would in a workload's store.
    native_plugins: HashMap<&'static str, Arc<dyn HostPlugin>>,
}

impl ComponentHostPluginState {
    /// A clone of the current capability sender, or `None` if the plugin is not
    /// running (before `start()` / after `stop()` / restart budget exhausted).
    fn sender(&self) -> Option<CapabilitySender> {
        self.tx.load_full().map(|tx| (*tx).clone())
    }

    /// The current incarnation's sender as a shared handle, whose `Arc` identity
    /// names the incarnation: a restart publishes a new `Arc`, so
    /// [`Arc::ptr_eq`] against a captured handle tells a deferred task whether
    /// the incarnation it spoke to is still the live one.
    fn sender_arc(&self) -> Option<Arc<CapabilitySender>> {
        self.tx.load_full()
    }

    /// Remove `workload_id` from the bound-workloads map without delivering an
    /// unbind. Used when a bind never reached the guest (its enqueue failed) or
    /// when its unbind is being deferred separately.
    fn forget_workload(&self, workload_id: &str) {
        self.bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(workload_id);
    }

    /// The current incarnation's job registry, or `None` if the plugin is not
    /// running. Read by the host `identity`/`cancel` imports.
    fn registry(&self) -> Option<Arc<JobRegistry>> {
        self.registry.load_full()
    }

    /// The per-call budget for a lifecycle (bind/unbind) delivery.
    fn lifecycle_timeout(&self) -> Duration {
        Duration::from_millis(self.lifecycle_timeout_ms.load(Ordering::Relaxed))
    }
}

/// A [`HostPlugin`] backed by a WebAssembly component running in its own
/// long-lived, supervised store.
pub struct ComponentHostPlugin {
    id: &'static str,
    engine: Engine,
    /// Pre-instantiated against a WASI linker; instantiates the plugin into a
    /// fresh store on each (re)start.
    pre: InstancePre<SharedCtx>,
    world: WitWorld,
    exports: Arc<Vec<ExportedInterface>>,
    /// Every exported function, flattened, for the TriggerService to resolve up front.
    capability_funcs: Vec<CapabilityFunc>,
    /// The plugin's `wasmcloud:host/workload-lifecycle` export, if it has one.
    lifecycle: Option<Arc<LifecycleFuncs>>,
    /// Hosts this plugin's own `wasi:http/outgoing-handler` calls may reach.
    /// Reinjected into every incarnation's own `Ctx` ([`build_plugin_store`]);
    /// enforced by the existing, unmodified `check_allowed_hosts`.
    allowed_hosts: Arc<[crate::host::allowed_hosts::AllowedHost]>,
    /// Names this plugin's own `wasi:sockets/ip-name-lookup` calls may
    /// resolve.
    allowed_ip_name_lookups: Arc<[crate::host::allowed_ip_name::AllowedIpName]>,
    /// What this plugin's own outgoing HTTP calls are sent through — the same
    /// handler a workload's outgoing calls use. `None` traps every call with
    /// "http client not available", matching today's behavior for a plugin
    /// that imports `wasi:http/outgoing-handler` with no handler configured.
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
    max_restarts: u32,
    state: Arc<ComponentHostPluginState>,
}

impl ComponentHostPlugin {
    /// Build a host component plugin from a compiled wasm `component` and the
    /// `engine` it will run on. `id` must be unique across the host's plugins.
    ///
    /// The component's exported interfaces become the capabilities this plugin
    /// provides. Fails if it exports no interface functions to serve.
    ///
    /// If the plugin *imports* an interface it also exports (a self-dependency,
    /// e.g. a recursive capability), those imports are wired on the plugin's own
    /// store to route back to the plugin itself — a re-entrant call chain the
    /// TriggerService's in-flight-task ceiling bounds.
    ///
    /// Any other capability import is resolved against `native_plugins` — the
    /// host's non-component `HostPlugin`s (`config`, `secrets`, `keyvalue`, and
    /// so on) — never against another component plugin, so a loading plugin can
    /// depend on host natives fully without ever forming an import cycle.
    /// `config` is this plugin's own resolved bind-time config, delivered to
    /// every native it imports the same way a workload's is.
    ///
    /// `allowed_hosts`/`allowed_ip_name_lookups` gate this plugin's own
    /// `wasi:http`/DNS egress the same way a workload's `LocalResources` do;
    /// `http_handler` is what its outgoing HTTP calls are actually sent
    /// through (typically the host's own, via `HostBuilder::http_handler`).
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        id: &'static str,
        wasm: &[u8],
        engine: Engine,
        native_plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
        config: &HashMap<String, String>,
        allowed_hosts: Arc<[crate::host::allowed_hosts::AllowedHost]>,
        allowed_ip_name_lookups: Arc<[crate::host::allowed_ip_name::AllowedIpName]>,
        http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
    ) -> anyhow::Result<Self> {
        // Defense-in-depth: re-filter to natives only. Both real call sites
        // already pass a pre-filtered map (`HostBuilder::native_plugins()`),
        // so this is a no-op today, but it makes the cycle-safety invariant
        // hold by construction here too, not just at the caller.
        let native_plugins = native_only(native_plugins);
        let state = Arc::new(ComponentHostPluginState {
            id,
            tx: ArcSwapOption::empty(),
            supervisor: Mutex::new(None),
            registry: ArcSwapOption::empty(),
            bound: Mutex::new(BTreeMap::new()),
            poison: Mutex::new(BTreeMap::new()),
            bind_trap_log: Mutex::new(Vec::new()),
            failure_sink: ArcSwapOption::empty(),
            lifecycle_timeout_ms: AtomicU64::new(
                crate::timeouts::plugin_lifecycle_call().as_millis() as u64,
            ),
            native_plugins: native_plugins.clone(),
        });

        let (exports, lifecycle, pre) =
            build_plugin_linker(&engine, id, wasm, &state, &native_plugins, config).await?;

        let world = WitWorld {
            imports: exports.iter().map(|e| e.wit.clone()).collect(),
            exports: Default::default(),
        };
        let mut capability_funcs: Vec<CapabilityFunc> = exports
            .iter()
            .flat_map(|e| {
                e.funcs.iter().map(|f| CapabilityFunc {
                    interface: Arc::clone(&e.name),
                    func: Arc::clone(&f.name),
                })
            })
            .collect();
        // The lifecycle hooks are served on the same instance as the
        // capabilities, so the TriggerService must resolve them too.
        if let Some(lifecycle) = &lifecycle {
            capability_funcs.push(CapabilityFunc {
                interface: Arc::clone(&lifecycle.interface),
                func: Arc::clone(&lifecycle.bind.name),
            });
            capability_funcs.push(CapabilityFunc {
                interface: Arc::clone(&lifecycle.interface),
                func: Arc::clone(&lifecycle.unbind.name),
            });
        }
        Ok(Self {
            id,
            engine,
            pre,
            world,
            exports: Arc::new(exports),
            capability_funcs,
            lifecycle: lifecycle.map(Arc::new),
            allowed_hosts,
            allowed_ip_name_lookups,
            http_handler,
            max_restarts: DEFAULT_MAX_RESTARTS,
            state,
        })
    }

    /// Override the number of supervised driver restarts before the plugin is
    /// declared dead (default [`DEFAULT_MAX_RESTARTS`]).
    pub fn with_max_restarts(mut self, max_restarts: u32) -> Self {
        self.max_restarts = max_restarts;
        self
    }

    /// Override the per-call budget for a lifecycle (bind/unbind) delivery
    /// (default [`crate::timeouts::plugin_lifecycle_call`]). Bounds how long a
    /// deploy or stop waits on a hook before failing and, for bind, deferring
    /// the rollback unbind.
    pub fn with_lifecycle_call_timeout(self, timeout: Duration) -> Self {
        self.state
            .lifecycle_timeout_ms
            .store(timeout.as_millis() as u64, Ordering::Relaxed);
        self
    }

    /// The workload ids whose `on-workload-bind` has been observed to trap, in
    /// harvest order — a diagnostic view of the fault-attribution the supervisor
    /// performs after each restart.
    pub fn bind_trap_log(&self) -> Vec<String> {
        self.state
            .bind_trap_log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|id| id.to_string())
            .collect()
    }

    /// Whether `workload_id` is still bound (present in the replay set). A
    /// workload evicted for a crash-looping bind is no longer bound.
    pub fn is_bound(&self, workload_id: &str) -> bool {
        self.state
            .bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(workload_id)
    }
}

/// Filters `plugins` down to the natives — every entry that is not itself a
/// [`ComponentHostPlugin`]. This is what a loading plugin's own capability
/// imports are resolved against ([`link_native_imports`]): natives only, never
/// another component plugin, so cycle-safety holds by construction rather than
/// by walking a dependency graph.
pub(crate) fn native_only(
    plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
) -> HashMap<&'static str, Arc<dyn HostPlugin>> {
    plugins
        .iter()
        .filter(|(_, p)| {
            (p.as_ref() as &dyn std::any::Any)
                .downcast_ref::<ComponentHostPlugin>()
                .is_none()
        })
        .map(|(k, v)| (*k, Arc::clone(v)))
        .collect()
}

/// Resolve a [`ComponentPluginSpec`] into a ready-to-register plugin: fetch its
/// wasm (OCI pull or local file), verify an optional digest pin, and build the
/// [`ComponentHostPlugin`]. The caller registers the result with
/// `HostBuilder::with_plugin` before `Host::start`; this does not start it.
///
/// `native_plugins` should be every native (non-component) plugin already
/// registered on the host — typically `HostBuilder::native_plugins()` — so
/// this plugin's own capability imports can resolve against them.
pub async fn load_component_plugin(
    spec: &ComponentPluginSpec,
    engine: &Engine,
    oci_config: OciConfig,
    native_plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
) -> anyhow::Result<Arc<ComponentHostPlugin>> {
    let loaded = spec
        .source
        .load_pinned(oci_config, spec.expected_digest.as_deref())
        .await
        .with_context(|| format!("loading host component plugin '{}'", spec.id))?;

    let id = intern_plugin_id(&spec.id);
    let mut plugin = ComponentHostPlugin::new(
        id,
        &loaded.bytes,
        engine.clone(),
        native_plugins,
        &spec.config,
        Arc::clone(&spec.allowed_hosts),
        Arc::clone(&spec.allowed_ip_name_lookups),
        http_handler,
    )
    .await
    .with_context(|| format!("failed to build host component plugin '{}'", spec.id))?;
    if let Some(max_restarts) = spec.max_restarts {
        plugin = plugin.with_max_restarts(max_restarts);
    }
    Ok(Arc::new(plugin))
}

/// Intern a config-supplied plugin id as `&'static str`, which is what a
/// [`HostPlugin`] id must be. Host component plugins load once at host start
/// from a bounded config and live for the whole process, so leaking these few
/// ids is acceptable and bounded. If plugins ever load dynamically on a running
/// host, the id must instead become an owned `Arc<str>` on the plugin so this
/// does not grow without bound.
fn intern_plugin_id(id: &str) -> &'static str {
    Box::leak(id.to_owned().into_boxed_str())
}

#[async_trait]
impl HostPlugin for ComponentHostPlugin {
    fn id(&self) -> &'static str {
        self.id
    }

    fn world(&self) -> WitWorld {
        self.world.clone()
    }

    fn set_workload_failure_sink(&self, sink: crate::plugin::WorkloadFailureSink) {
        self.state.failure_sink.store(Some(Arc::new(sink)));
    }

    async fn start(&self) -> anyhow::Result<()> {
        let (tx, rx) = tokio::sync::mpsc::channel(CAPABILITY_CHANNEL_CAPACITY);
        // Publish the sender and snapshot the bound workloads atomically (see
        // [`replay_snapshot`]); leftover binds (a stop()/start() cycle) replay,
        // while binds arriving from now on deliver through the channel.
        let replay = {
            let bound = self
                .state
                .bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.state.tx.store(Some(Arc::new(tx)));
            replay_snapshot(&bound, self.lifecycle.as_deref())
        };

        let supervisor = tokio::spawn(run_supervisor(
            self.engine.clone(),
            self.pre.clone(),
            self.capability_funcs.clone(),
            self.lifecycle.clone(),
            Arc::clone(&self.state),
            self.max_restarts,
            replay,
            rx,
            Arc::clone(&self.allowed_hosts),
            Arc::clone(&self.allowed_ip_name_lookups),
            self.http_handler.clone(),
        ));
        *self
            .state
            .supervisor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(supervisor);
        debug!(id = self.id, "started host component plugin");
        Ok(())
    }

    async fn on_workload_bind(
        &self,
        workload: &UnresolvedWorkload,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let Some(lifecycle) = &self.lifecycle else {
            return Ok(());
        };
        let info = workload_info_val(workload, &interfaces);
        let workload_id: Arc<str> = Arc::from(workload.id());
        // Record the workload and read the current sender (as an incarnation
        // handle) under one `bound` lock: the supervisor swaps incarnations
        // under the same lock (see [`replay_snapshot`]), so this bind is either
        // in the next replay snapshot (and this send targets the previous, dead
        // channel) or delivered through the live channel — never both.
        // Recording before delivery also means a restart DURING the call
        // replays the bind (the hook is idempotent); rolled back below on
        // failure.
        let sender_arc = {
            let mut bound = self
                .state
                .bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            bound.insert(Arc::clone(&workload_id), info.clone());
            self.state.sender_arc()
        };
        let Some(sender_arc) = sender_arc else {
            self.state.forget_workload(&workload_id);
            return Err(anyhow::anyhow!(
                "host component plugin '{}' is not running",
                self.id
            ));
        };

        // Enqueue the bind. A send timeout / closed channel means it never
        // reached the guest (tokio's bounded send does not enqueue a dropped
        // send), so no hook is running — forget the workload and fail, no
        // unbind needed.
        let reply_rx = match send_lifecycle_job(
            &sender_arc,
            &self.state,
            lifecycle,
            &lifecycle.bind,
            info,
            &workload_id,
        )
        .await
        {
            Ok(reply_rx) => reply_rx,
            Err(e) => {
                self.state.forget_workload(&workload_id);
                return Err(e.context(format!(
                    "failed to deliver workload bind to host component plugin '{}'",
                    self.id
                )));
            }
        };

        // Await the reply, but keep the receiver if it times out: we cannot
        // cancel a running guest hook, so on timeout the bind is still in
        // flight. Fail the deploy now and defer the rollback unbind until the
        // hook actually returns — guaranteeing bind-then-unbind ordering so
        // whatever it provisions late is still reclaimed (see
        // [`spawn_deferred_unbind`]).
        let failure = match await_bind_reply(reply_rx, &self.state).await {
            BindReply::Completed(Ok(results)) => match decode_bind_reply(&results) {
                Ok(Ok(())) => {
                    debug!(id = self.id, %workload_id, "workload bound to host component plugin");
                    return Ok(());
                }
                Ok(Err(msg)) => anyhow::anyhow!(
                    "host component plugin '{}' rejected workload '{workload_id}': {msg}",
                    self.id
                ),
                Err(e) => anyhow::Error::from(e).context(format!(
                    "host component plugin '{}' returned a malformed on-workload-bind reply",
                    self.id
                )),
            },
            BindReply::Completed(Err(e)) => e.context(format!(
                "failed to deliver workload bind to host component plugin '{}'",
                self.id
            )),
            BindReply::TimedOut(pending) => {
                self.state.forget_workload(&workload_id);
                spawn_deferred_unbind(
                    sender_arc,
                    pending,
                    Arc::clone(&workload_id),
                    Arc::clone(lifecycle),
                    Arc::clone(&self.state),
                );
                return Err(anyhow::anyhow!(
                    "on-workload-bind to host component plugin '{}' timed out; deploy failed, \
                     cleanup deferred until the hook returns",
                    self.id
                ));
            }
        };

        // The bind returned (rejection, malformed reply, or a delivery error
        // after the hook ran): the hook is no longer in flight, so roll back
        // immediately with a best-effort unbind.
        if let Err(e) = remove_and_unbind(&self.state, lifecycle, &workload_id).await {
            warn!(id = self.id, %workload_id, err = %e, "post-failure unbind not delivered");
        }
        Err(failure)
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let workload_id: Arc<str> = Arc::from(item.workload_id());
        let linker = item.linker();

        for exported in self.exports.iter() {
            let iface_names: Vec<&str> =
                exported.wit.interfaces.iter().map(String::as_str).collect();
            // Only wire interfaces this workload was actually matched on.
            if !interfaces.contains(&exported.wit.namespace, &exported.wit.package, &iface_names) {
                continue;
            }

            if let Err(e) = add_capabilities_to_linker(linker, &self.state, exported) {
                // The engine's bind-failure cleanup only unbinds plugins whose
                // item binds ALL succeeded — a plugin failing its own item bind
                // is not yet on that list — so roll back the bind delivered in
                // `on_workload_bind` ourselves.
                if let Some(lifecycle) = &self.lifecycle
                    && let Err(unbind_err) =
                        remove_and_unbind(&self.state, lifecycle, &workload_id).await
                {
                    warn!(
                        id = self.id,
                        %workload_id,
                        err = %unbind_err,
                        "item-bind rollback unbind not delivered"
                    );
                }
                return Err(e);
            }
            debug!(id = self.id, interface = %exported.name, "wired host component capability");
        }

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let Some(lifecycle) = &self.lifecycle else {
            return Ok(());
        };
        // Best-effort by design: the workload is going away regardless, and if
        // the plugin is stopped or restarting its per-workload state is already
        // gone with the store.
        let workload_id: Arc<str> = Arc::from(workload_id);
        match remove_and_unbind(&self.state, lifecycle, &workload_id).await {
            Ok(()) => {
                debug!(id = self.id, %workload_id, "workload unbound from host component plugin");
            }
            Err(e) => {
                warn!(
                    id = self.id,
                    %workload_id,
                    err = %e,
                    "failed to deliver workload unbind to host component plugin (best-effort)"
                );
            }
        }
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        // Clearing the sender closes the current incarnation's channel, ending
        // the TriggerService's serve loop and letting the supervisor exit cleanly; the
        // registry goes with it (the driver's tasks retire their jobs as they end).
        // Cleared under the `bound` lock so it cannot interleave with the
        // supervisor's restart republish, which re-checks the sender under the
        // same lock — otherwise a republish racing this clear could leave a
        // "stopped" plugin with a live sender.
        {
            let _bound = self
                .state
                .bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.state.tx.store(None);
        }
        self.state.registry.store(None);
        let supervisor = self
            .state
            .supervisor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(mut handle) = supervisor
            && tokio::time::timeout(crate::timeouts::plugin_stop(), &mut handle)
                .await
                .is_err()
        {
            // The supervisor is wedged (e.g. a driver hung on an in-flight call
            // whose shim still holds a channel sender). Abort it so it cannot
            // survive `stop()` and later — after a fresh `start()` — mistake the
            // new incarnation for a fault and restart a duplicate store.
            warn!(
                id = self.id,
                "host component plugin supervisor did not stop in time; aborting it"
            );
            handle.abort();
        }
        debug!(id = self.id, "stopped host component plugin");
        Ok(())
    }
}

/// A WIT package that's part of the WASI base linked unconditionally into
/// every plugin store by [`Engine::prepare_host_component`] (`wasi:io`,
/// `wasi:filesystem`, `wasi:clocks`, `wasi:random`, `wasi:cli`,
/// `wasi:sockets`), plus `wasi:http` (linked whenever the component uses it).
/// An import in one of these packages is never a candidate for native-builtin
/// resolution below — it's already satisfied.
fn is_base_wasi(wit: &WitInterface) -> bool {
    wit.namespace == "wasi"
        && matches!(
            wit.package.as_str(),
            "io" | "filesystem" | "clocks" | "random" | "cli" | "sockets" | "http"
        )
}

/// Resolve a plugin's remaining unsatisfied imports against the host's native
/// (non-component) plugins — `config`, `secrets`, `keyvalue`, and so on — and
/// wire them into `linker`. Delivers this plugin's own resolved bind-time
/// `config` to whichever natives it imports via the same `on-workload-bind`
/// hook a workload gets, by representing the plugin as a synthetic
/// single-component workload purely to reuse `bind_plugins`'s matching and
/// rollback logic — never by writing `config` to a file the plugin reads.
///
/// `native_plugins` must already exclude every [`ComponentHostPlugin`] (this
/// plugin's own natural self-imports are handled separately, before this
/// runs): a loading plugin may depend on host natives fully, but never on
/// another component plugin, so there is no cycle to detect, only natives to
/// resolve.
async fn link_native_imports(
    engine: &Engine,
    id: &str,
    component: &Component,
    linker: Linker<SharedCtx>,
    already_linked: &std::collections::HashSet<Arc<str>>,
    native_plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
    config: &HashMap<String, String>,
) -> anyhow::Result<Linker<SharedCtx>> {
    // Merge same-instance imports (e.g. `wasmcloud:secrets/store` and
    // `wasmcloud:secrets/reveal` are two separate introspected imports but one
    // logical binding) before attaching `config`, mirroring
    // `WorkloadMetadata::world()`'s own import merge — otherwise each would
    // carry its own copy of the plugin's whole config and collide as two
    // bindings setting the same keys.
    let mut merged: HashMap<String, WitInterface> = HashMap::new();
    for imported in introspect_imports(component)? {
        if already_linked.contains(&imported.name) {
            continue;
        }
        let wit = imported.wit;
        if is_reserved(&wit) || is_base_wasi(&wit) {
            continue;
        }
        merged
            .entry(wit.instance())
            .and_modify(|existing| {
                existing.merge(&wit);
            })
            .or_insert(wit);
    }
    let native_imports: Vec<WitInterface> = merged
        .into_values()
        .map(|mut wit| {
            wit.config = config.clone();
            wit
        })
        .collect();
    if native_imports.is_empty() {
        return Ok(linker);
    }

    let mut plugin_component = WorkloadComponent::new(
        id,
        id,
        id,
        id,
        component.clone(),
        linker,
        Vec::new(),
        LocalResources::default(),
        Arc::new(std::sync::Mutex::new(loopback::Network::default())),
        InstancePolicy::Ephemeral,
    );
    // `WorkloadComponent::new` always mints its own fresh random component
    // id, the same as it would for a real workload component instance. But
    // this synthetic component stands in for the plugin's own real running
    // store, which self-identifies as `id` (`build_plugin_store` below) —
    // and natives that key bind-time state by component id (e.g.
    // `DynamicConfig`'s `wasi:config` store) must see that same `id` here,
    // or the plugin's own runtime lookups miss everything resolved at bind
    // time.
    let component_id: Arc<str> = Arc::from(id);
    plugin_component.metadata.id = Arc::clone(&component_id);

    let mut synthetic =
        UnresolvedWorkload::new(id, id, id, None, [plugin_component], native_imports);
    synthetic
        .bind_plugins(native_plugins)
        .await
        .with_context(|| {
            format!("failed to resolve native capability imports for plugin '{id}'")
        })?;

    let mut plugin_component = synthetic
        .take_component(&component_id)
        .context("plugin's own synthetic workload component vanished during binding")?;
    Ok(std::mem::replace(
        plugin_component.metadata.linker(),
        Linker::new(engine.inner()),
    ))
}

/// Build the plugin store's linker and pre-instantiate the component against it.
/// This is the single place that declares the plugin's whole import surface:
///
/// - the WASI (and `wasi:http`) base, from [`Engine::prepare_host_component`];
/// - the `wasmcloud:host/identity` import (unused unless the plugin imports it);
/// - a route back to the plugin's own capability channel for any interface it
///   both imports and exports (a self-import);
/// - every other capability import, resolved against the host's native
///   plugins ([`link_native_imports`]) — never against another component
///   plugin.
///
/// The introspected exports are partitioned: the reserved `wasmcloud:host`
/// lifecycle interface is a host-invoked contract, while everything else is a
/// capability workloads may import. Returns the capability exports and the
/// lifecycle export (if any) alongside the [`InstancePre`].
async fn build_plugin_linker(
    engine: &Engine,
    id: &str,
    wasm: &[u8],
    state: &Arc<ComponentHostPluginState>,
    native_plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
    config: &HashMap<String, String>,
) -> anyhow::Result<(
    Vec<ExportedInterface>,
    Option<LifecycleFuncs>,
    InstancePre<SharedCtx>,
)> {
    let (component, mut linker) = engine.prepare_host_component(wasm)?;
    let mut lifecycle = None;
    let mut exports = Vec::new();
    for export in introspect_exports(&component)? {
        if is_reserved(&export.wit) {
            // Matched by interface name, not the exact `export.name` string —
            // `wasmcloud:host` is versioned as one package, so a patch bump
            // anywhere in it must not break a plugin built against the
            // previous patch version of `workload-lifecycle` it already
            // exports.
            anyhow::ensure!(
                export.wit.interfaces.contains(HOST_LIFECYCLE_EXPORT),
                "host component plugin '{id}' exports reserved host interface {}",
                export.name
            );
            lifecycle = Some(lifecycle_funcs(id, export)?);
        } else {
            exports.push(export);
        }
    }
    anyhow::ensure!(
        exports.iter().any(|e| !e.funcs.is_empty()),
        "host component plugin '{id}' exports no capability functions to serve"
    );

    install_host_identity(&mut linker, state)
        .with_context(|| format!("failed to install host identity on plugin '{id}'"))?;
    install_host_cancel(&mut linker, state)
        .with_context(|| format!("failed to install host cancel on plugin '{id}'"))?;

    let mut self_linked = std::collections::HashSet::new();
    for imported in introspect_imports(&component)? {
        if exports.iter().any(|e| e.name == imported.name) {
            add_capabilities_to_linker(&mut linker, state, &imported).with_context(|| {
                format!(
                    "failed to wire self-import {} on plugin '{id}'",
                    imported.name
                )
            })?;
            self_linked.insert(Arc::clone(&imported.name));
        }
    }

    let linker = link_native_imports(
        engine,
        id,
        &component,
        linker,
        &self_linked,
        native_plugins,
        config,
    )
    .await?;

    linker
        .instantiate_pre(&component)
        .map_err(anyhow::Error::from)
        .context("failed to pre-instantiate host component plugin")
        .map(|pre| (exports, lifecycle, pre))
}

/// Interface name of the host identity import a plugin may use to partition
/// state by caller.
const HOST_IDENTITY_INTERFACE: &str = "wasmcloud:host/identity@0.1.0";

/// The caller's root guest task, or `None` if it can't be determined.
///
/// [`StoreContextMut::async_call_stack`] only errors in unusual states (e.g.
/// called outside a guest invocation); treat that as "no caller" but leave a
/// trace so the degradation isn't silent. Its last item is the root call.
fn caller_root_task(store: &mut StoreContextMut<'_, SharedCtx>) -> Option<GuestTaskId> {
    match store.async_call_stack() {
        Ok(stack) => stack.last(),
        Err(e) => {
            debug!(err = %e, "async call stack unavailable; treating as no caller task");
            None
        }
    }
}

/// Install the `wasmcloud:host/identity` import on the plugin's own linker: two
/// no-argument funcs returning the workload/component id of the caller whose
/// capability call is currently running. Each walks its async call stack back to
/// the root export task and looks that task up in the current incarnation's
/// [`JobRegistry`], so the answer is exact even while calls from other workloads
/// interleave. A plugin that imports the interface can thereby partition its state
/// per caller; a plugin that does not import it leaves these definitions unused.
fn install_host_identity(
    linker: &mut Linker<SharedCtx>,
    state: &Arc<ComponentHostPluginState>,
) -> anyhow::Result<()> {
    let mut instance = linker
        .instance(HOST_IDENTITY_INTERFACE)
        .map_err(|e| e.context("failed to open the host identity linker instance"))?;

    let workload_state = Arc::clone(state);
    instance
        .func_new(
            "get-workload-id",
            move |mut store, _ty, _params, results| {
                let root = caller_root_task(&mut store);
                let id = root
                    .and_then(|task| workload_state.registry()?.caller_for_task(task))
                    .map(|c| c.workload_id.to_string())
                    .unwrap_or_default();
                if let Some(slot) = results.first_mut() {
                    *slot = Val::String(id);
                }
                Ok(())
            },
        )
        .map_err(|e| e.context("failed to define wasmcloud:host/identity#get-workload-id"))?;
    let component_state = Arc::clone(state);
    instance
        .func_new(
            "get-component-id",
            move |mut store, _ty, _params, results| {
                let root = caller_root_task(&mut store);
                // `get-component-id` returns a bare `string`, so "there is no
                // component" has no representation of its own — a lifecycle
                // hook (`component_id: None`) and an unresolvable caller both
                // collapse to the empty string here, at the WIT boundary and
                // nowhere earlier. Widening this to `option<string>` would
                // break the released `wasmcloud:host/identity`; until then a
                // plugin must read `workload-info` inside a hook rather than
                // ask who is calling.
                let id = root
                    .and_then(|task| component_state.registry()?.caller_for_task(task))
                    .and_then(|c| c.component_id.clone())
                    .map(|id| id.to_string())
                    .unwrap_or_default();
                if let Some(slot) = results.first_mut() {
                    *slot = Val::String(id);
                }
                Ok(())
            },
        )
        .map_err(|e| e.context("failed to define wasmcloud:host/identity#get-component-id"))?;
    Ok(())
}

/// Interface name of the host cancel import a plugin may use to cancel one of its
/// own in-flight invocations.
const HOST_CANCEL_INTERFACE: &str = "wasmcloud:host/cancel@0.1.0";

/// Install the `wasmcloud:host/cancel` import on the plugin's own linker:
/// `current-job` returns the job the caller runs under (or `0`), `request-cancel`
/// marks a job when the requester shares its owner's workload, and `is-cancelled`
/// lets the running guest poll its own job. All resolve the caller's root guest
/// task against the current incarnation's [`JobRegistry`]; a plugin that does not
/// import the interface leaves them unused. See the module docs for the
/// cooperative-cancellation model.
fn install_host_cancel(
    linker: &mut Linker<SharedCtx>,
    state: &Arc<ComponentHostPluginState>,
) -> anyhow::Result<()> {
    let mut instance = linker
        .instance(HOST_CANCEL_INTERFACE)
        .map_err(|e| e.context("failed to open the host cancel linker instance"))?;

    let current_state = Arc::clone(state);
    instance
        .func_new("current-job", move |mut store, _ty, _params, results| {
            let root = caller_root_task(&mut store);
            let job = root
                .and_then(|task| current_state.registry()?.job_for_task(task))
                .unwrap_or(0);
            if let Some(slot) = results.first_mut() {
                *slot = Val::U64(job);
            }
            Ok(())
        })
        .map_err(|e| e.context("failed to define wasmcloud:host/cancel#current-job"))?;
    let cancel_state = Arc::clone(state);
    instance
        .func_new("request-cancel", move |mut store, _ty, params, results| {
            let job = match params.first() {
                Some(Val::U64(job)) => *job,
                _ => wasmtime::bail!("request-cancel expects a single u64 job id"),
            };
            let root = caller_root_task(&mut store);
            let accepted = match (root, cancel_state.registry()) {
                (Some(task), Some(registry)) => match registry.caller_for_task(task) {
                    Some(requester) => registry.request_cancel(job, &requester),
                    None => false,
                },
                _ => false,
            };
            if let Some(slot) = results.first_mut() {
                *slot = Val::Bool(accepted);
            }
            Ok(())
        })
        .map_err(|e| e.context("failed to define wasmcloud:host/cancel#request-cancel"))?;
    let is_cancelled_state = Arc::clone(state);
    instance
        .func_new("is-cancelled", move |mut store, _ty, _params, results| {
            let root = caller_root_task(&mut store);
            let cancelled = root
                .and_then(|task| {
                    let registry = is_cancelled_state.registry()?;
                    let job = registry.job_for_task(task)?;
                    Some(registry.is_cancelled(job))
                })
                .unwrap_or(false);
            if let Some(slot) = results.first_mut() {
                *slot = Val::Bool(cancelled);
            }
            Ok(())
        })
        .map_err(|e| e.context("failed to define wasmcloud:host/cancel#is-cancelled"))?;
    Ok(())
}

/// Add this plugin's capability for `iface` to `linker` — the cross-store
/// counterpart of a Rust `HostPlugin`'s `add_to_linker`. Where `add_to_linker`
/// installs host functions that run in the caller's own store, these
/// `func_new_concurrent` shims route each call across the store boundary to the
/// plugin's persistent store via the channel held by `state`. Used on a
/// workload's linker ([`ComponentHostPlugin::on_workload_item_bind`]) and on the
/// plugin's own linker for self-imports ([`build_plugin_linker`]).
fn add_capabilities_to_linker(
    linker: &mut Linker<SharedCtx>,
    state: &Arc<ComponentHostPluginState>,
    iface: &ExportedInterface,
) -> anyhow::Result<()> {
    let mut linker_instance = linker
        .instance(&iface.name)
        .map_err(|e| e.context(format!("failed to open linker instance {}", iface.name)))?;

    // Register each resource the interface defines as a cross-store proxy. A
    // caller holds an opaque proxy; dropping it here routes a drop of the real
    // resource back to the plugin store. (Methods/constructors/statics are
    // ordinary functions, installed below.)
    for resource in &iface.resources {
        let state = Arc::clone(state);
        linker_instance
            .resource_concurrent(
                resource.as_ref(),
                resource_bridge::proxy_resource_type(),
                move |accessor, rep| {
                    let state = Arc::clone(&state);
                    Box::pin(async move { drop_proxy_resource(accessor, &state, rep).await })
                },
            )
            .map_err(|e| {
                e.context(format!(
                    "failed to register proxied resource {}/{}",
                    iface.name, resource
                ))
            })?;
    }

    for func in &iface.funcs {
        let state = Arc::clone(state);
        let interface = Arc::clone(&iface.name);
        let func_name = Arc::clone(&func.name);
        let param_tys = Arc::clone(&func.param_tys);
        let result_tys = Arc::clone(&func.result_tys);

        linker_instance
            .func_new_concurrent(
                func.name.as_ref(),
                move |accessor, _func_ty, params: &[Val], results: &mut [Val]| {
                    let state = Arc::clone(&state);
                    let interface = Arc::clone(&interface);
                    let func = Arc::clone(&func_name);
                    let param_tys = Arc::clone(&param_tys);
                    let result_tys = Arc::clone(&result_tys);
                    Box::pin(async move {
                        route_capability_call(
                            accessor, &state, interface, func, params, &param_tys, result_tys,
                            results,
                        )
                        .await
                    })
                },
            )
            .map_err(|e| {
                e.context(format!(
                    "failed to install capability shim for {}/{}",
                    iface.name, func.name
                ))
            })?;
    }
    Ok(())
}

/// A caller dropped its proxy for a plugin resource: read the proxy's id out of
/// the caller table, then route a drop of the real resource to the plugin store.
async fn drop_proxy_resource(
    accessor: &Accessor<SharedCtx>,
    state: &ComponentHostPluginState,
    rep: u32,
) -> wasmtime::Result<()> {
    let proxy_id = accessor.with(|mut access| -> wasmtime::Result<u64> {
        let res = Resource::<ProxyResource>::new_own(rep);
        Ok(access.data_mut().table.delete(res)?.proxy_id)
    })?;

    // Best-effort: if the plugin was stopped or restarted, the real resource is
    // already gone, so a closed channel here is not an error.
    if let Some(sender) = state.sender() {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if sender
            .send(CapabilityJob::DropResource {
                proxy_id,
                reply: reply_tx,
            })
            .await
            .is_ok()
        {
            let _ = reply_rx.await;
        }
    }
    Ok(())
}

/// Route one capability call from a workload store to the persistent plugin
/// store.
///
/// Arguments are extracted in the caller store (handle-free values copied;
/// `stream`/`future` handles relocated), sent over the current incarnation's
/// channel, and the plugin's relocated results are injected back into the
/// caller's results slots. The `extract → await → inject` ordering keeps each
/// `Accessor::with` borrow in a discrete sync block, never held across the
/// await.
#[allow(clippy::too_many_arguments)]
async fn route_capability_call(
    accessor: &Accessor<SharedCtx>,
    state: &ComponentHostPluginState,
    interface: Arc<str>,
    func: Arc<str>,
    params: &[Val],
    param_tys: &[Type],
    result_tys: Arc<[Type]>,
    results: &mut [Val],
) -> wasmtime::Result<()> {
    // Read the calling workload's identity (for per-caller state partitioning)
    // and extract the arguments in the caller store (handle-free values copied;
    // `stream`/`future` handles relocated), in one discrete sync block. Any
    // argument-stream pumps run under the caller's (long-lived) runtime, so their
    // drain signals are dropped here. Runaway re-entrant recursion is bounded by
    // the TriggerService's in-flight-task ceiling, not here.
    let (caller, args) = accessor.with(
        |mut access| -> wasmtime::Result<(CallerIdentity, Vec<Relocated>)> {
            let caller = {
                let ctx = &access.data_mut().active_ctx;
                CallerIdentity {
                    workload_id: Arc::clone(&ctx.workload_id),
                    component_id: Some(Arc::clone(&ctx.component_id)),
                }
            };
            let mut dones = Vec::new();
            let mut out = Vec::with_capacity(params.len());
            for (val, ty) in params.iter().zip(param_tys.iter()) {
                out.push(relocate::extract(
                    access.as_context_mut(),
                    val,
                    ty,
                    &mut dones,
                )?);
            }
            Ok((caller, out))
        },
    )?;

    let sender = state.sender().ok_or_else(|| {
        wasmtime::format_err!("host component plugin '{}' is not running", state.id)
    })?;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    sender
        .send(CapabilityJob::Call(CapabilityCall {
            interface,
            func,
            caller,
            args,
            result_tys,
            reply: reply_tx,
        }))
        .await
        .map_err(|_| {
            wasmtime::format_err!("host component plugin '{}' channel closed", state.id)
        })?;

    let produced = reply_rx.await.map_err(|_| {
        wasmtime::format_err!("host component plugin '{}' dropped the reply", state.id)
    })??;

    // Inject the relocated results into the caller store.
    accessor.with(|mut access| -> wasmtime::Result<()> {
        for (slot, relocated) in results.iter_mut().zip(produced) {
            *slot = relocate::inject(access.as_context_mut(), relocated)?;
        }
        Ok(())
    })
}

/// Supervise the plugin's persistent driver: (re)build the store, spawn the
/// TriggerService, and await the driver. A clean shutdown (the sender cleared by
/// `stop()`) exits; a fault restarts up to `max_restarts` times, handing each
/// new incarnation a fresh channel whose sender the installed shims pick up.
#[allow(clippy::too_many_arguments)]
async fn run_supervisor(
    engine: Engine,
    pre: InstancePre<SharedCtx>,
    funcs: Vec<CapabilityFunc>,
    lifecycle: Option<Arc<LifecycleFuncs>>,
    state: Arc<ComponentHostPluginState>,
    max_restarts: u32,
    mut replay: Vec<LifecycleReplay>,
    mut rx: tokio::sync::mpsc::Receiver<CapabilityJob>,
    allowed_hosts: Arc<[crate::host::allowed_hosts::AllowedHost]>,
    allowed_ip_name_lookups: Arc<[crate::host::allowed_ip_name::AllowedIpName]>,
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
) {
    let mut restarts = 0u32;
    loop {
        let store = build_plugin_store(
            &engine,
            state.id,
            &state.native_plugins,
            &allowed_hosts,
            &allowed_ip_name_lookups,
            http_handler.clone(),
        );
        // A fresh job registry per incarnation, published on `state` so the
        // baked-in identity/cancel imports reach this store's live jobs. Stale
        // jobs from a faulted incarnation die with its store (their guards retire
        // as the tasks drop).
        let registry = JobRegistry::new();
        state.registry.store(Some(Arc::clone(&registry)));
        // `replay` was snapshotted when this incarnation's channel was
        // published (in `start()` or the restart path below): the incarnation
        // rebuilds its per-workload state from it before serving any queued
        // capability call (the TriggerService completes the replay before
        // reading the channel).
        let ingress = Ingress::Capability {
            funcs: funcs.clone(),
            rx,
            registry,
            replay: std::mem::take(&mut replay),
        };
        let trigger_service = TriggerService::spawn(store, pre.clone(), vec![ingress]);

        // The driver runs until the capability channel closes (clean shutdown)
        // or the store faults (e.g. a guest trap).
        let started = tokio::time::Instant::now();
        let _ = trigger_service.driver.await;
        let uptime = started.elapsed();

        // `stop()` clears the sender; if it is gone, this was a clean shutdown.
        if state.sender().is_none() {
            debug!(id = state.id, "host component plugin driver stopped");
            state.registry.store(None);
            return;
        }

        // A driver that stayed up for a while before faulting gets a fresh
        // budget — only rapid crash loops should exhaust it. The poison strikes
        // reset on the same signal, so a transient bind failure spread across
        // healthy periods never accrues to eviction.
        let healthy = uptime >= crate::timeouts::plugin_healthy_uptime();
        if healthy {
            restarts = 0;
            state
                .poison
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }

        // Attribute the fault. A fault during replay names the workload whose
        // bind crash-looped the shared store (via the serve-loop marker, still
        // reachable in the faulted registry). That is the workload's own poison,
        // so it strikes that workload — evicting at the ceiling so the next
        // incarnation stops replaying it and recovers for every other tenant —
        // and does NOT consume the plugin-wide restart budget. Only an
        // unattributed fault (a serving-phase trap, or a deploy bind trap) is
        // charged to the budget.
        // Only a plugin that exports the lifecycle interface replays binds, so
        // only it can have a replay fault to attribute.
        let culprit = if lifecycle.is_some() {
            attribute_replay_fault(&state)
        } else {
            None
        };
        if let Some(workload_id) = culprit {
            let strikes = {
                let mut poison = state
                    .poison
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let strikes = poison.entry(Arc::clone(&workload_id)).or_insert(0);
                *strikes += 1;
                *strikes
            };
            warn!(
                id = state.id,
                %workload_id,
                strikes,
                "on-workload-bind trapped during replay; struck the workload"
            );
            if strikes >= POISON_EVICT_STRIKES {
                evict_workload(&state, &workload_id);
                report_workload_failed(&state, &workload_id);
            }
        } else {
            if restarts >= max_restarts {
                error!(
                    id = state.id,
                    restarts, "host component plugin exceeded its restart budget; giving up"
                );
                state.tx.store(None);
                state.registry.store(None);
                return;
            }
            restarts += 1;
        }

        // Fresh channel for the new incarnation; installed shims read the new
        // sender via `state.sender()` on their next call, and calls made during
        // the backoff below queue on it rather than failing. Published under
        // the `bound` lock, atomically with the replay snapshot (see
        // [`replay_snapshot`] for why).
        let (new_tx, new_rx) = tokio::sync::mpsc::channel(CAPABILITY_CHANNEL_CAPACITY);
        {
            let bound = state
                .bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The sender was live when the fault was observed; if it is gone
            // now, `stop()` cleared it (under this same lock) in the window
            // since — republishing would undo the stop and leave a zombie
            // incarnation running past `stop()`.
            if state.sender().is_none() {
                debug!(
                    id = state.id,
                    "host component plugin stopped during fault handling"
                );
                return;
            }
            state.tx.store(Some(Arc::new(new_tx)));
            replay = replay_snapshot(&bound, lifecycle.as_deref());
        }
        rx = new_rx;

        // Back off before restarting so a store that faults instantly (e.g. a
        // component that traps on instantiation) cannot spin the budget away in a
        // tight, delay-free loop.
        let backoff = crate::timeouts::plugin_restart_backoff_max().min(
            std::time::Duration::from_millis(200u64.saturating_mul(u64::from(restarts))),
        );
        warn!(
            id = state.id,
            restarts,
            backoff_ms = backoff.as_millis() as u64,
            "restarting host component plugin driver after backoff"
        );
        tokio::time::sleep(backoff).await;

        // `stop()` may have run during the backoff.
        if state.sender().is_none() {
            debug!(
                id = state.id,
                "host component plugin stopped during restart backoff"
            );
            return;
        }
    }
}

/// Build the plugin's own store with a minimal host-scoped context. The plugin
/// is not part of any workload; its `workload_id`/`component_id` are just its
/// own id. Carries the native plugins this plugin's own imports resolved
/// against ([`link_native_imports`]), so a native's host function — reading
/// its own plugin state via `Ctx::try_get_plugin` — finds it here the same way
/// it would in a workload's store.
#[allow(clippy::too_many_arguments)]
fn build_plugin_store(
    engine: &Engine,
    id: &'static str,
    native_plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
    allowed_hosts: &Arc<[crate::host::allowed_hosts::AllowedHost]>,
    allowed_ip_name_lookups: &Arc<[crate::host::allowed_ip_name::AllowedIpName]>,
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
) -> Store<SharedCtx> {
    // Same policy a non-service workload component gets: DNS lookup gated by
    // `allowed_ip_name_lookups`, `wasi:http` gated by `allowed_hosts` (via
    // `Ctx::with_allowed_hosts` + the existing `check_allowed_hosts`), raw
    // socket connect otherwise unrestricted — a plugin never binds a listen
    // socket, so `TcpBind`/`UdpBind` are always denied.
    let sockets_ctx = crate::sockets::WasiSocketsCtx {
        socket_addr_check: crate::sockets::SocketAddrCheck::new(move |_addr, reason| {
            Box::pin(async move {
                use crate::sockets::SocketAddrUse;
                !matches!(reason, SocketAddrUse::TcpBind | SocketAddrUse::UdpBind)
            })
        }),
        loopback: Arc::new(std::sync::Mutex::new(
            crate::sockets::loopback::Network::default(),
        )),
        allowed_ip_name_lookups: Arc::clone(allowed_ip_name_lookups),
        ..Default::default()
    };

    let mut ctx_builder = Ctx::builder(id, id)
        .with_plugins(
            native_plugins
                .iter()
                .map(|(k, v)| (*k, Arc::clone(v) as Arc<dyn HostPlugin + Send + Sync>))
                .collect(),
        )
        .with_sockets(sockets_ctx)
        .with_allowed_hosts(Arc::clone(allowed_hosts));
    if let Some(http_handler) = http_handler {
        ctx_builder = ctx_builder.with_http_handler(http_handler);
    }
    let ctx = ctx_builder.build();
    // The registry marks this as the plugin (real) side of the resource bridge
    // and keeps the resources it hands out across the boundary alive.
    Store::new(engine.inner(), SharedCtx::new(ctx).with_resource_registry())
}

/// Introspect a plugin component's exported interfaces and their functions from
/// its component type.
fn introspect_exports(component: &Component) -> anyhow::Result<Vec<ExportedInterface>> {
    let ty = component.component_type();
    introspect_interfaces(component, ty.exports(component.engine()))
}

/// Introspect a plugin component's *imported* interfaces and their functions.
/// Used to wire self-imports (an interface the plugin both imports and exports)
/// back to the plugin's own capability channel.
fn introspect_imports(component: &Component) -> anyhow::Result<Vec<ExportedInterface>> {
    let ty = component.component_type();
    introspect_interfaces(component, ty.imports(component.engine()))
}

/// Collect the capability interfaces (and their functions and resource types)
/// from one side of a component's type — its exports or its imports. A
/// top-level func not inside an interface is not a capability we route, so it
/// is skipped.
fn introspect_interfaces<'a>(
    component: &Component,
    items: impl Iterator<Item = (&'a str, wasmtime::component::types::ComponentExtern<'a>)>,
) -> anyhow::Result<Vec<ExportedInterface>> {
    let engine = component.engine();
    let mut interfaces = Vec::new();

    for (iface_name, iface_item) in items {
        // A plain import's `iface_name` IS its `namespace:package/interface`
        // path, so parsing it directly works. A `(implements ..)`-labeled
        // import's `iface_name` is just the label (e.g. "db-password") — the
        // component-model type carries the real interface it implements
        // separately, in `implements`, which is what must be parsed instead,
        // with the label preserved as `WitInterface.name` (the routing key
        // multiplexing plugins match on).
        let implements = iface_item.implements;

        let ComponentItem::ComponentInstance(instance_ty) = iface_item.ty else {
            continue;
        };

        let mut funcs = Vec::new();
        let mut resources = Vec::new();
        for (func_name, func_item) in instance_ty.exports(engine) {
            match func_item.ty {
                ComponentItem::ComponentFunc(func_ty) => funcs.push(ExportedFunc {
                    name: func_name.into(),
                    param_tys: func_ty.params().map(|(_, ty)| ty).collect(),
                    result_tys: func_ty.results().collect(),
                }),
                ComponentItem::Resource(_) => resources.push(func_name.into()),
                _ => {}
            }
        }

        let wit = match implements {
            Some(target) => {
                let mut wit = WitInterface::from(target);
                wit.name = Some(iface_name.to_string());
                wit
            }
            None => WitInterface::from(iface_name),
        };
        interfaces.push(ExportedInterface {
            name: iface_name.into(),
            wit,
            funcs,
            resources,
        });
    }

    Ok(interfaces)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;

    /// Records the `WorkloadItem::id()` it's bound under — the same accessor
    /// `DynamicConfig` (`wasi:config/store`) keys its per-component config
    /// store by — instead of actually serving a capability. Standing in for
    /// any id-keyed native so this test isn't coupled to `DynamicConfig`'s
    /// internals.
    #[derive(Default)]
    struct IdRecordingPlugin {
        seen_id: Mutex<Option<String>>,
    }

    #[async_trait]
    impl HostPlugin for IdRecordingPlugin {
        fn id(&self) -> &'static str {
            "id-recording-test-plugin"
        }

        fn world(&self) -> WitWorld {
            WitWorld {
                imports: HashSet::from([WitInterface::from("test:probe/marker@0.1.0")]),
                exports: HashSet::new(),
            }
        }

        async fn on_workload_item_bind<'a>(
            &self,
            item: &mut WorkloadItem<'a>,
            _interfaces: WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            *self.seen_id.lock().unwrap() = Some(item.id().to_string());
            Ok(())
        }
    }

    /// Regression test for a synthetic bind component keyed by a fresh random
    /// UUID instead of the plugin's own id: `link_native_imports` builds a
    /// synthetic single-component workload purely to reuse `bind_plugins`'s
    /// matching, but the plugin's own real running store self-identifies as
    /// `id` (`build_plugin_store`). Any native that keys bind-time state by
    /// component id (e.g. `DynamicConfig`'s `wasi:config` store) must see
    /// that same `id` here, or the plugin's own capability calls miss
    /// everything resolved at bind time.
    #[tokio::test]
    async fn link_native_imports_binds_natives_under_the_plugin_id() {
        let plugin_id = "id-recording-test-plugin-instance";
        let wat = r#"
            (component
                (import "test:probe/marker@0.1.0" (instance))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("failed to parse WAT");
        let engine = Engine::builder().build().expect("failed to build engine");
        let component = Component::new(engine.inner(), &wasm).expect("failed to compile");
        let linker = Linker::new(engine.inner());

        let recorder = Arc::new(IdRecordingPlugin::default());
        let native_plugins: HashMap<&'static str, Arc<dyn HostPlugin>> =
            HashMap::from([(recorder.id(), Arc::clone(&recorder) as Arc<dyn HostPlugin>)]);
        let config = HashMap::from([("key".to_string(), "value".to_string())]);

        link_native_imports(
            &engine,
            plugin_id,
            &component,
            linker,
            &HashSet::new(),
            &native_plugins,
            &config,
        )
        .await
        .expect("link_native_imports should succeed");

        assert_eq!(
            recorder.seen_id.lock().unwrap().as_deref(),
            Some(plugin_id),
            "native plugin must see the plugin's own id, not a synthetic bind-time UUID"
        );
    }
}
