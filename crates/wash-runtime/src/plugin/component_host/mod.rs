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
//!
//! Calls also run the other way. A plugin that imports
//! `wasmcloud:host/workload-call` may *import* an interface no host built-in
//! provides, in which case a workload that exports it satisfies it — the
//! arrangement `wasi:http` and `wasmcloud:messaging` already have, where the
//! host both serves a workload's imports and calls the handler it exports.
//! Such an interface must be `async func` throughout, because the shim the host
//! installs for it is concurrent and a sync-typed import cannot bind to one,
//! and every function of it must return a `result` the host can build an error
//! into — a call out to another workload's guest can trap or stop mid-call, and
//! the plugin's store is shared by every workload it serves, so the host
//! answers with a value instead of faulting it.
//! [`classify_workload_imports`] decides which imports those are, and the
//! `workload_call` submodule holds the rest: the per-workload routes (claimed in
//! `on_workload_resolved`, exactly as a native plugin claims them), the
//! `wasmcloud:host/workload-call` `target` handle a plugin uses to name which
//! workload a call goes to, and the fallback that sends an unaddressed call back
//! to the workload whose capability call is being served.

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
use crate::engine::workload::{
    ResolvedWorkload, UnresolvedWorkload, WorkloadComponent, WorkloadItem,
};
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
mod workload_call;

use lifecycle::{
    BindReply, HOST_LIFECYCLE_EXPORT, LifecycleFuncs, POISON_EVICT_STRIKES, attribute_replay_fault,
    await_bind_reply, evict_workload, lifecycle_funcs, remove_and_unbind, replay_snapshot,
    report_workload_failed, send_lifecycle_job, spawn_deferred_unbind, workload_info_val,
};
use workload_call::{WorkloadCalls, add_workload_calls_to_linker, install_host_workload};

/// Capacity of a plugin incarnation's capability-call channel. Bounds queued
/// (not-yet-served) calls; in-flight (being-served) calls are separately capped
/// by the TriggerService's per-store in-flight-task ceiling.
const CAPABILITY_CHANNEL_CAPACITY: usize = 256;

/// Default number of times a plugin's driver is restarted under supervision
/// before the plugin is declared dead. One store now serves every workload, so
/// a restart story is required rather than optional.
const DEFAULT_MAX_RESTARTS: u32 = 3;

pub(super) type CapabilitySender = tokio::sync::mpsc::Sender<CapabilityJob>;

/// One exported capability function, introspected from the plugin component's
/// type at construction. The param/result types drive the relocation pass that
/// moves arguments and results across the store boundary.
struct ExportedFunc {
    name: Arc<str>,
    param_tys: Arc<[Type]>,
    result_tys: Arc<[Type]>,
    /// Whether the function is declared `async func`. Every shim the host
    /// installs is concurrent, and async-ness is part of a function's type
    /// identity — a concurrent definition against a sync-typed import fails to
    /// link with "type mismatch with async" — so a workload-facing import is
    /// refused unless it is declared `async`
    /// ([`workload_call::add_workload_calls_to_linker`]).
    is_async: bool,
}

/// One interface, with the functions and resource types it carries, as
/// introspected from a plugin component's type. Used for all three kinds the
/// plugin deals in — a capability it exports, an interface it both imports and
/// exports (a self-import), and an interface it imports for a *workload* to
/// export ([`workload_call`]) — because the shape the host needs is the same
/// for each: the instance name to address it by, its functions' names and
/// types, and any resources it defines.
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
    /// The other direction: the interfaces this plugin imports for a *workload*
    /// to export, which workloads currently serve them, and which workload each
    /// in-flight guest task is addressing. Empty for a plugin that only
    /// provides capabilities. See [`workload_call`].
    workload_calls: WorkloadCalls,
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
    /// Ports this plugin declared, as the operator wrote them.
    /// The subset of `ports` this plugin binds for real itself, precomputed for
    /// the per-incarnation socket policy.
    direct_binds: Arc<[crate::sockets::policy::DirectBind]>,
    /// The host's one port table, so a collision with another plugin (or, later,
    /// a workload) is caught at `start()` with both holders named.
    /// Handle to the current incarnation's virtual network. Published listeners
    /// hold this rather than a network, which is what lets them stay bound
    /// across a supervised restart.
    network: crate::host::ports::NetworkHandle,
    /// The host-level half of this plugin's socket policy.
    socket_policy: Arc<crate::sockets::policy::SocketPolicy>,
    state: Arc<ComponentHostPluginState>,
}

#[bon::bon]
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
    ///
    /// `native_plugins`, `config`, `allowed_hosts`, `allowed_ip_name_lookups`,
    /// and `http_handler` all default to empty/`None` — most callers (tests
    /// especially) only care about a handful of these.
    #[builder(finish_fn = build)]
    pub async fn new(
        id: &'static str,
        wasm: &[u8],
        engine: Engine,
        #[builder(default)] native_plugins: HashMap<&'static str, Arc<dyn HostPlugin>>,
        #[builder(default)] config: HashMap<String, String>,
        #[builder(default)] allowed_hosts: Arc<[crate::host::allowed_hosts::AllowedHost]>,
        #[builder(default)] allowed_ip_name_lookups: Arc<
            [crate::host::allowed_ip_name::AllowedIpName],
        >,
        http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
        #[builder(default)] ports: Arc<[crate::host::declared_port::DeclaredPort]>,
        socket_policy: Option<Arc<crate::sockets::policy::SocketPolicy>>,
    ) -> anyhow::Result<Self> {
        crate::host::declared_port::validate_ports(&ports, &format!("host plugin '{id}'"))?;
        let direct_binds = ports
            .iter()
            .filter_map(|port| match port.mode() {
                crate::host::declared_port::PortMode::Direct { bind } => {
                    Some(crate::sockets::policy::DirectBind {
                        addr: core::net::SocketAddr::new(bind, port.port),
                        udp: port.protocol.is_udp(),
                    })
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        // Defense-in-depth: re-filter to natives only. Both real call sites
        // already pass a pre-filtered map (`HostBuilder::native_plugins()`),
        // so this is a no-op today, but it makes the cycle-safety invariant
        // hold by construction here too, not just at the caller.
        let native_plugins = native_only(&native_plugins);
        let (component, base_linker) = engine.prepare_host_component(wasm)?;
        let (exports, lifecycle) = introspect_capability_exports(id, &component)?;
        let workload_imports =
            classify_workload_imports(id, &component, &exports, &native_plugins)?;
        // A plugin has to participate in one direction or the other: serve a
        // capability a workload imports, or call an interface a workload
        // exports. A plugin doing only the latter is a legitimate shape — a
        // trigger that dispatches from its own `wasi:cli/run` and exports no
        // capability at all.
        anyhow::ensure!(
            exports.iter().any(|e| !e.funcs.is_empty()) || !workload_imports.is_empty(),
            "host component plugin '{id}' exports no capability functions to serve and imports \
             no interface for a workload to export"
        );

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
            workload_calls: WorkloadCalls::new(id, workload_imports),
        });

        let pre = build_plugin_linker(
            &engine,
            id,
            &component,
            base_linker,
            &exports,
            &state,
            &native_plugins,
            &config,
        )
        .await?;

        // A capability the plugin exports and an interface it calls on a
        // workload are both entries in the world the host matches a workload
        // against: `includes_bidirectional` already covers a workload that
        // satisfies one by *exporting* it, so the two directions need no
        // distinction here.
        let world = WitWorld {
            imports: exports
                .iter()
                .chain(state.workload_calls.imports())
                .map(|e| e.wit.clone())
                .collect(),
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
            direct_binds: Arc::from(direct_binds),
            network: crate::host::ports::NetworkHandle::new(),
            socket_policy: socket_policy.unwrap_or_default(),
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

    /// Wire one workload item against the interfaces it matched this plugin on:
    /// check the ones this plugin *calls*, and install shims for the ones it
    /// *serves*. Every failure is returned rather than handled, so
    /// [`HostPlugin::on_workload_item_bind`] can roll the workload's bind back
    /// on any of them from one place.
    fn wire_workload_item<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // An interface this plugin calls puts nothing on the item's linker —
        // the item provides it rather than consuming it. But a match on such an
        // interface only means the item's world mentions the package, so check
        // here that it really exports it: an item that imports it instead has
        // nobody to serve it, and would otherwise fail later as an unresolved
        // wasmtime import with no mention of this plugin.
        for called in self.state.workload_calls.imports() {
            let iface_names: Vec<&str> = called.wit.interfaces.iter().map(String::as_str).collect();
            if !interfaces.contains(&called.wit.namespace, &called.wit.package, &iface_names) {
                continue;
            }
            // An entry naming its package alone matches every interface in it,
            // this one included, so an item may be here without touching the
            // interface at all. Only an item that does has anything to answer
            // for below.
            if !item.world().uses(&called.wit) {
                continue;
            }
            anyhow::ensure!(
                item.world()
                    .exports
                    .iter()
                    .any(|exported| exported.contains(&called.wit)),
                "host component plugin '{}' calls {} rather than serving it, and no host built-in \
                 serves it either, so '{}' must export it — but component '{}' imports it instead",
                self.id,
                called.name,
                item.workload_id(),
                item.id(),
            );
        }

        let linker = item.linker();
        for exported in self.exports.iter() {
            let iface_names: Vec<&str> =
                exported.wit.interfaces.iter().map(String::as_str).collect();
            // Only wire interfaces this workload was actually matched on.
            if !interfaces.contains(&exported.wit.namespace, &exported.wit.package, &iface_names) {
                continue;
            }
            add_capabilities_to_linker(linker, &self.state, exported)?;
            debug!(id = self.id, interface = %exported.name, "wired host component capability");
        }
        Ok(())
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
///
/// `publish` governs this plugin's declared `ports`. Pass a context carrying the
/// *same* table for every plugin on a host — it is the one place that knows
/// which real ports are taken, so separate tables would let two plugins each
/// believe they own the same address. `None` gives a private table and
/// publishing disabled, which is what a test or an embedder that exposes no
/// ports wants.
pub async fn load_component_plugin(
    spec: &ComponentPluginSpec,
    engine: &Engine,
    oci_config: OciConfig,
    native_plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
    socket_policy: Option<Arc<crate::sockets::policy::SocketPolicy>>,
) -> anyhow::Result<Arc<ComponentHostPlugin>> {
    let loaded = spec
        .source
        .load_pinned(oci_config, spec.expected_digest.as_deref())
        .await
        .with_context(|| format!("loading host component plugin '{}'", spec.id))?;

    let id = intern_plugin_id(&spec.id);
    let mut plugin = ComponentHostPlugin::builder()
        .id(id)
        .wasm(&loaded.bytes)
        .engine(engine.clone())
        .native_plugins(native_plugins.clone())
        .config(spec.config.clone())
        .allowed_hosts(Arc::clone(&spec.allowed_hosts))
        .allowed_ip_name_lookups(Arc::clone(&spec.allowed_ip_name_lookups))
        .maybe_http_handler(http_handler)
        .maybe_socket_policy(socket_policy)
        .build()
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
            self.network.clone(),
            Arc::clone(&self.direct_binds),
            Arc::clone(&self.socket_policy),
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
        let (reply_rx, call) = match send_lifecycle_job(
            &sender_arc,
            &self.state,
            lifecycle,
            &lifecycle.bind,
            info,
            &workload_id,
        )
        .await
        {
            Ok(sent) => sent,
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
        let failure = match await_bind_reply(reply_rx, call, &self.state).await {
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
        let Err(e) = self.wire_workload_item(item, interfaces) else {
            return Ok(());
        };
        // However this item bind failed, the bind delivered in
        // `on_workload_bind` has to come back off: the engine's bind-failure
        // cleanup only unbinds plugins whose item binds ALL succeeded, and a
        // plugin failing its own item bind is not yet on that list. Left
        // undone, the workload stays in the replay set and its bind is
        // re-delivered to every later incarnation of a plugin it never
        // deployed against.
        if let Some(lifecycle) = &self.lifecycle
            && let Err(unbind_err) = remove_and_unbind(&self.state, lifecycle, &workload_id).await
        {
            warn!(
                id = self.id,
                %workload_id,
                err = %unbind_err,
                "item-bind rollback unbind not delivered"
            );
        }
        Err(e)
    }

    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        if self.state.workload_calls.is_empty() {
            return Ok(());
        }
        self.state
            .workload_calls
            .register(workload, component_id)
            .await
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // Drop the routes into this workload first: its stores and warm
        // instances go away with it, and an unbind hook below may run for a
        // while.
        self.state.workload_calls.unregister(workload_id);

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
    // No operator bindings: a plugin's own native imports are configured by the
    // plugin entry's `config`, delivered through `on-workload-bind`, not by a
    // workload naming a binding.
    synthetic
        .bind_plugins(native_plugins, &crate::plugin::PluginBindings::new())
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

/// Partition a plugin component's exports: the reserved `wasmcloud:host`
/// lifecycle interface is a host-invoked contract, while everything else is a
/// capability workloads may import.
fn introspect_capability_exports(
    id: &str,
    component: &Component,
) -> anyhow::Result<(Vec<ExportedInterface>, Option<LifecycleFuncs>)> {
    let mut lifecycle = None;
    let mut exports = Vec::new();
    for export in introspect_exports(component)? {
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
    Ok((exports, lifecycle))
}

/// Whether an import is already accounted for by something other than a
/// workload: an interface the plugin exports itself (a self-import), the WASI
/// base, or the reserved `wasmcloud:host` package.
fn is_self_satisfied(imported: &ExportedInterface, exports: &[ExportedInterface]) -> bool {
    exports.iter().any(|e| e.name == imported.name)
        || is_reserved(&imported.wit)
        || is_base_wasi(&imported.wit)
}

/// Whether the plugin declares that it calls workloads, by importing
/// `wasmcloud:host/workload-call`.
///
/// This is what opens its import surface: only a plugin that says it calls
/// workloads may have an otherwise-unsatisfiable import answered by a workload
/// export. Without it every import must resolve to the plugin's own exports,
/// the WASI base, or a native — and one that does not fails the load in
/// [`link_native_imports`], naming the missing built-in.
///
/// Matched on the interface name within the reserved package rather than the
/// exact import string, so a `wasmcloud:host` patch bump does not change what a
/// plugin built against the previous one declares.
fn declares_workload_calls(imports: &[ExportedInterface]) -> bool {
    imports.iter().any(|imported| {
        is_reserved(&imported.wit) && imported.wit.interfaces.contains("workload-call")
    })
}

/// The plugin's imports that a *workload* must export, in the order the
/// component declares them.
///
/// An import lands here only when the plugin [`declares_workload_calls`] and
/// nothing else can satisfy it: it is not [`is_self_satisfied`], and no native
/// plugin *provides* it. Natives win, so a built-in's interface is never handed
/// to a workload instead.
///
/// The native check is per import and asks what a native provides (its world's
/// imports), not what it mentions. That is what lets a plugin importing both
/// `wasmcloud:messaging/consumer` and `handler` get `consumer` from the native
/// and `handler` from a workload — the split the native messaging plugin itself
/// has.
///
/// An import no workload ever exports is reported to the plugin as
/// `not-exported` on the call that needs it, naming the interface.
fn classify_workload_imports(
    id: &str,
    component: &Component,
    exports: &[ExportedInterface],
    native_plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
) -> anyhow::Result<Vec<ExportedInterface>> {
    let imports = introspect_imports(component)?;
    if !declares_workload_calls(&imports) {
        return Ok(Vec::new());
    }
    let native_worlds: Vec<WitWorld> = native_plugins.values().map(|p| p.world()).collect();

    let mut workload_imports = Vec::new();
    for imported in imports {
        if is_self_satisfied(&imported, exports)
            || native_worlds
                .iter()
                .any(|world| world.provides(&imported.wit))
        {
            continue;
        }
        anyhow::ensure!(
            imported.wit.name.is_none(),
            "host component plugin '{id}' imports {} under an `(implements ..)` label, but no \
             host built-in serves it, so a workload export would have to — and a workload's \
             export carries no label to route by. Import the interface directly instead.",
            imported.name
        );
        anyhow::ensure!(
            imported.resources.is_empty(),
            "host component plugin '{id}' imports {} for a workload to export, but the interface \
             defines resource types, which cannot cross the boundary between a plugin's store \
             and a workload's",
            imported.name
        );
        // The interface may define no resource of its own and still name one in
        // a signature — a `borrow<session>` from a sibling `types` interface, or
        // a `wasi:io` stream. Classify every function the way the call itself
        // will be classified, so an unbridgeable signature is one clear failure
        // when the plugin loads rather than a deploy failure in every workload
        // that exports the interface.
        for func in &imported.funcs {
            anyhow::ensure!(
                crate::engine::linked_call::types_are_bridge_safe(&func.param_tys)
                    && crate::engine::linked_call::types_are_bridge_safe(&func.result_tys),
                "host component plugin '{id}' imports {}#{} for a workload to export, but its \
                 signature carries a handle that cannot cross the boundary between a plugin's \
                 store and a workload's; only plain values, `stream<T>`, and `future<T>` can",
                imported.name,
                func.name
            );
            anyhow::ensure!(
                workload_call::error_shape(&func.result_tys).is_some(),
                "host component plugin '{id}' imports {}#{} for a workload to export, but it \
                 returns nothing the host can report a failure through. Such a call reaches \
                 another workload's guest, which can trap or stop mid-call, and the host will \
                 not take the plugin down for it — so the signature has to be able to say so. \
                 Return a `result` whose error arm the host can build: \
                 `wasmcloud:host/types.{{call-error}}` for the structured form (recommended), a \
                 plain `string`, or the interface's own error type so long as one of its cases \
                 carries nothing, a `string`, or an `option<string>`.",
                imported.name,
                func.name
            );
        }
        debug!(
            id,
            interface = %imported.name,
            "host component plugin import will be served by a workload export"
        );
        workload_imports.push(imported);
    }
    Ok(workload_imports)
}

/// Wire the plugin store's linker and pre-instantiate the component against it.
/// This is the single place that declares the plugin's whole import surface:
///
/// - the WASI (and `wasi:http`) base, from [`Engine::prepare_host_component`];
/// - the `wasmcloud:host` identity/cancel/workload imports (unused unless the
///   plugin imports them);
/// - a route back to the plugin's own capability channel for any interface it
///   both imports and exports (a self-import);
/// - a route out to a workload's export for every import
///   [`classify_workload_imports`] found no other provider for;
/// - every other capability import, resolved against the host's native
///   plugins ([`link_native_imports`]) — never against another component
///   plugin.
#[allow(clippy::too_many_arguments)]
async fn build_plugin_linker(
    engine: &Engine,
    id: &str,
    component: &Component,
    mut linker: Linker<SharedCtx>,
    exports: &[ExportedInterface],
    state: &Arc<ComponentHostPluginState>,
    native_plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
    config: &HashMap<String, String>,
) -> anyhow::Result<InstancePre<SharedCtx>> {
    install_host_identity(&mut linker, state)
        .with_context(|| format!("failed to install host identity on plugin '{id}'"))?;
    install_host_cancel(&mut linker, state)
        .with_context(|| format!("failed to install host cancel on plugin '{id}'"))?;
    install_host_workload(&mut linker, state)
        .with_context(|| format!("failed to install host workload targeting on plugin '{id}'"))?;

    let mut linked = std::collections::HashSet::new();
    for imported in introspect_imports(component)? {
        if exports.iter().any(|e| e.name == imported.name) {
            add_capabilities_to_linker(&mut linker, state, &imported).with_context(|| {
                format!(
                    "failed to wire self-import {} on plugin '{id}'",
                    imported.name
                )
            })?;
            linked.insert(Arc::clone(&imported.name));
        }
    }
    for imported in state.workload_calls.imports() {
        add_workload_calls_to_linker(&mut linker, state, imported).with_context(|| {
            format!(
                "failed to wire workload-served import {} on plugin '{id}'",
                imported.name
            )
        })?;
        linked.insert(Arc::clone(&imported.name));
    }

    let linker = link_native_imports(
        engine,
        id,
        component,
        linker,
        &linked,
        native_plugins,
        config,
    )
    .await?;

    linker
        .instantiate_pre(component)
        .map_err(anyhow::Error::from)
        .context("failed to pre-instantiate host component plugin")
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
    // Deadline enforced here, in the calling workload's task, outside the
    // plugin's store (see `crate::engine::abandon`). The plugin store is
    // `WarnThenTrap`: an abandoned call is logged at the grace and only traps
    // the shared store at the escalation.
    let call = crate::engine::abandon::DispatchedCall::new(
        "capability (plugin)",
        crate::timeouts::plugin_capability_call(),
    );
    sender
        .send(CapabilityJob::Call(CapabilityCall {
            interface,
            func,
            caller,
            args,
            result_tys,
            reply: reply_tx,
            abandoned: call.flag(),
        }))
        .await
        .map_err(|_| {
            wasmtime::format_err!("host component plugin '{}' channel closed", state.id)
        })?;

    let produced = call
        .await_reply(reply_rx)
        .await
        .ok_or_else(|| {
            wasmtime::format_err!(
                "host component plugin '{}' produced no reply in time",
                state.id
            )
        })?
        .map_err(|_| {
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
    network: crate::host::ports::NetworkHandle,
    direct_binds: Arc<[crate::sockets::policy::DirectBind]>,
    socket_policy: Arc<crate::sockets::policy::SocketPolicy>,
) {
    let mut restarts = 0u32;
    loop {
        // Installs this incarnation's virtual network on the handle, which is
        // how a listener published before this incarnation existed finds it.
        let store = build_plugin_store(
            &engine,
            state.id,
            &state.native_plugins,
            &allowed_hosts,
            &allowed_ip_name_lookups,
            http_handler.clone(),
            &network,
            &direct_binds,
            &socket_policy,
        );
        // A fresh job registry per incarnation, published on `state` so the
        // baked-in identity/cancel imports reach this store's live jobs. Stale
        // jobs from a faulted incarnation die with its store (their guards retire
        // as the tasks drop).
        let registry = JobRegistry::new();
        state.registry.store(Some(Arc::clone(&registry)));
        // Target handles are per-store, but the stacks recording them live on
        // `state` and so outlive a faulted incarnation, whose guests never ran
        // their destructors. Guest task ids are reused once their task ends, so
        // a stranded entry would otherwise route some later task's unaddressed
        // calls to a workload it never named.
        state.workload_calls.clear_targets();
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
            network.clear();
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
                // No further incarnation will register a listener. Any still-open
                // published listener now fails its readiness window rather than
                // polling a network nothing will ever bind in.
                network.clear();
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
    network: &crate::host::ports::NetworkHandle,
    direct_binds: &Arc<[crate::sockets::policy::DirectBind]>,
    socket_policy: &Arc<crate::sockets::policy::SocketPolicy>,
) -> Store<SharedCtx> {
    // DNS lookup gated by `allowed_ip_name_lookups`, `wasi:http` gated by
    // `allowed_hosts` (via `Ctx::with_allowed_hosts` + the existing
    // `check_allowed_hosts`), raw socket connect otherwise unrestricted. Binds
    // land in the plugin's own private virtual network unless the operator
    // declared a concrete address for this plugin to hold directly — see
    // `crate::sockets::policy::SocketPolicy`.
    let policy = Arc::new(crate::sockets::policy::SocketPolicy {
        allowed_hosts: Arc::clone(allowed_hosts),
        ..socket_policy.for_guest(
            crate::sockets::policy::GuestKind::Plugin {
                direct_binds: Arc::clone(direct_binds),
            },
            id,
        )
    });
    // A fresh network per incarnation, published on the handle so a
    // `PublishedPort` bound before this incarnation existed splices into it.
    // It must be fresh: tearing down a store does not release the virtual ports
    // its sockets registered, so reusing one would fail the next incarnation's
    // bind with `AddressInUse`.
    let loopback = network.replace();
    let sockets_ctx = crate::sockets::WasiSocketsCtx {
        socket_addr_check: crate::sockets::SocketAddrCheck::new(move |addr, reason| {
            let policy = Arc::clone(&policy);
            Box::pin(async move { policy.decide(reason, addr) })
        }),
        loopback,
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
    // A host component plugin is a guest too: its linear memory is charged to
    // the same host-wide budget the workloads on this engine draw on.
    let mut store = Store::new(
        engine.inner(),
        SharedCtx::new(ctx)
            .with_resource_registry()
            .with_guest_memory(engine.guest_memory()),
    );
    // The other half of the same requirement: on a fuel-metering engine a store
    // starts with no fuel, and calling a guest without fuel traps. See
    // `new_store_from_templates`, which does this for every workload store.
    let _ = store.set_fuel(u64::MAX);
    // Required, not optional: the engine enables `epoch_interruption`, and a
    // store that never sets a deadline traps the moment it runs any guest code.
    // `WarnThenTrap` because this one store serves every workload that imports
    // the plugin's capability: an abandoned call gets a long runway to finish
    // on its own before ending it costs every tenant a supervised restart.
    crate::engine::abandon::arm_epoch_deadline(
        &mut store,
        crate::engine::abandon::AbandonedCallPolicy::WarnThenTrap,
    );
    crate::engine::guest_memory::install_memory_limiter(&mut store);
    store
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
                    is_async: func_ty.async_(),
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

    /// Compile `wat` and classify its imports the way plugin construction
    /// does, against a single native serving `test:probe/marker`.
    fn workload_facing(wat: &str, exports: &[ExportedInterface]) -> anyhow::Result<Vec<String>> {
        let wasm = wat::parse_str(wat).expect("failed to parse WAT");
        let engine = Engine::builder().build().expect("failed to build engine");
        let component = Component::new(engine.inner(), &wasm).expect("failed to compile");
        let recorder = Arc::new(IdRecordingPlugin::default());
        let native_plugins: HashMap<&'static str, Arc<dyn HostPlugin>> =
            HashMap::from([(recorder.id(), recorder as Arc<dyn HostPlugin>)]);
        Ok(
            classify_workload_imports("test-plugin", &component, exports, &native_plugins)?
                .into_iter()
                .map(|imported| imported.name.to_string())
                .collect(),
        )
    }

    /// The `wasmcloud:host/workload-call` import every opted-in plugin
    /// declares, as a WAT line to paste into a test component.
    const DECLARES_WORKLOAD_CALLS: &str =
        r#"(import "wasmcloud:host/workload-call@0.1.3" (instance))"#;

    /// Only an import nothing else can satisfy becomes the workload's to
    /// export. A native serving the interface wins — so introducing a built-in
    /// never silently turns into a call out to a workload — and the WASI base
    /// is already linked.
    #[test]
    fn only_unsatisfiable_imports_are_left_to_a_workload() {
        let wat = format!(
            r#"
            (component
                {DECLARES_WORKLOAD_CALLS}
                (import "acme:events/handler@0.1.0" (instance))
                (import "test:probe/marker@0.1.0" (instance))
                (import "wasi:clocks/monotonic-clock@0.3.0" (instance))
            )
        "#
        );
        assert_eq!(
            workload_facing(&wat, &[]).expect("classification should succeed"),
            vec!["acme:events/handler@0.1.0"],
            "the native-served and base-WASI imports should not be left to a workload"
        );
    }

    /// A plugin that never says it calls workloads keeps a closed import
    /// surface: an import no native serves is not quietly answered by a
    /// workload export, it stays unresolved for `link_native_imports` to fail
    /// the load on, naming the missing built-in.
    #[test]
    fn a_plugin_that_does_not_declare_workload_calls_demotes_nothing() {
        let wat = r#"
            (component
                (import "acme:events/handler@0.1.0" (instance))
                (import "wasi:keyvalue/store@0.2.0-draft" (instance))
            )
        "#;
        assert!(
            workload_facing(wat, &[])
                .expect("classification should succeed")
                .is_empty(),
            "without the wasmcloud:host/workload-call import nothing may be left to a workload"
        );
    }

    /// A native covering part of a package wins for the part it covers, and
    /// leaves the rest to a workload. Asking about the merged package instead
    /// would find no native covering both and hand the whole package over,
    /// including the half the built-in was serving.
    #[test]
    fn a_partly_native_package_splits_rather_than_demoting_whole() {
        let wat = format!(
            r#"
            (component
                {DECLARES_WORKLOAD_CALLS}
                (import "test:probe/marker@0.1.0" (instance))
                (import "test:probe/callback@0.1.0" (instance))
            )
        "#
        );
        assert_eq!(
            workload_facing(&wat, &[]).expect("classification should succeed"),
            vec!["test:probe/callback@0.1.0"],
            "the interface the native serves must stay with the native"
        );
    }

    /// An interface the plugin exports itself stays a self-import, routed back
    /// to its own capability channel rather than out to a workload.
    #[test]
    fn a_self_import_is_not_left_to_a_workload() {
        let wat = format!(
            r#"
            (component
                {DECLARES_WORKLOAD_CALLS}
                (import "acme:events/handler@0.1.0" (instance))
            )
        "#
        );
        let exports = vec![ExportedInterface {
            name: Arc::from("acme:events/handler@0.1.0"),
            wit: WitInterface::from("acme:events/handler@0.1.0"),
            funcs: Vec::new(),
            resources: Vec::new(),
        }];
        assert!(
            workload_facing(&wat, &exports)
                .expect("classification should succeed")
                .is_empty(),
            "an interface the plugin also exports is served by the plugin itself"
        );
    }

    /// A resource handle has no representation on the far side of a
    /// plugin/workload store boundary, so an interface defining one is refused
    /// when the plugin loads rather than trapping on a call much later.
    #[test]
    fn a_resource_carrying_import_is_refused() {
        let wat = format!(
            r#"
            (component
                {DECLARES_WORKLOAD_CALLS}
                (import "acme:events/handler@0.1.0" (instance
                    (export "session" (type (sub resource)))
                ))
            )
        "#
        );
        let err = workload_facing(&wat, &[]).expect_err("a resource import should be refused");
        assert!(
            err.to_string().contains("resource types"),
            "the error should name the reason, got: {err}"
        );
    }

    /// A resource the interface does not define itself but names in a
    /// signature is refused too — the guard reads the functions, not just the
    /// interface's own type definitions, so an unbridgeable signature is one
    /// clear failure at load rather than a deploy failure in every workload
    /// that exports the interface.
    #[test]
    fn an_imported_resource_in_a_signature_is_refused() {
        let wat = format!(
            r#"
            (component
                {DECLARES_WORKLOAD_CALLS}
                (import "acme:events/types@0.1.0" (instance $types
                    (export "session" (type (sub resource)))
                ))
                (alias export $types "session" (type $session))
                (import "acme:events/handler@0.1.0" (instance
                    (export "notify" (func (param "s" (own $session))))
                ))
            )
        "#
        );
        let err = workload_facing(&wat, &[]).expect_err("a resource parameter should be refused");
        assert!(
            err.to_string().contains("cannot cross"),
            "the error should name the reason, got: {err}"
        );
    }

    /// A workload-facing import must be able to say a call failed, because the
    /// host answers a failed call with a value rather than trapping the plugin.
    /// A function with no such result is refused when the plugin loads.
    #[test]
    fn a_workload_facing_import_that_cannot_report_failure_is_refused() {
        let wat = format!(
            r#"
            (component
                {DECLARES_WORKLOAD_CALLS}
                (import "acme:events/handler@0.1.0" (instance
                    (export "notify" (func async (param "m" string) (result string)))
                ))
            )
        "#
        );
        let err =
            workload_facing(&wat, &[]).expect_err("an import with no error arm should be refused");
        let msg = err.to_string();
        assert!(
            msg.contains("error arm") && msg.contains("call-error"),
            "the error should name the fix, got: {msg}"
        );
    }

    /// `result<_, string>` is accepted, which is what an off-the-shelf interface
    /// like `wasmcloud:messaging/handler` already declares.
    #[test]
    fn a_workload_facing_import_returning_a_string_error_is_accepted() {
        let wat = format!(
            r#"
            (component
                {DECLARES_WORKLOAD_CALLS}
                (import "acme:events/handler@0.1.0" (instance
                    (export "notify" (func async (param "m" string)
                        (result (result string (error string)))))
                ))
            )
        "#
        );
        assert_eq!(
            workload_facing(&wat, &[]).expect("classification should succeed"),
            vec!["acme:events/handler@0.1.0"],
        );
    }

    /// A plugin's own state, with nothing running — enough to install linker
    /// shims, which is all the workload-facing import tests need.
    pub(super) fn idle_state(id: &'static str) -> Arc<ComponentHostPluginState> {
        Arc::new(ComponentHostPluginState {
            id,
            tx: ArcSwapOption::empty(),
            supervisor: Mutex::new(None),
            registry: ArcSwapOption::empty(),
            bound: Mutex::new(BTreeMap::new()),
            poison: Mutex::new(BTreeMap::new()),
            bind_trap_log: Mutex::new(Vec::new()),
            failure_sink: ArcSwapOption::empty(),
            lifecycle_timeout_ms: AtomicU64::new(1_000),
            native_plugins: HashMap::new(),
            workload_calls: WorkloadCalls::new(id, Vec::new()),
        })
    }

    /// The one interface `wat` declares, introspected. Written as an import
    /// because WAT can state an import's type without implementing it, and
    /// introspection produces the same [`ExportedInterface`] either way. The way
    /// to get a real [`Type`] into a test: wasmtime mints them, nothing else can.
    pub(super) fn introspected_interface(wat: &str) -> ExportedInterface {
        let wasm = wat::parse_str(wat).expect("failed to parse WAT");
        let engine = Engine::builder().build().expect("failed to build engine");
        let component = Component::new(engine.inner(), &wasm).expect("failed to compile");
        introspect_imports(&component)
            .expect("introspection should succeed")
            .pop()
            .expect("the WAT should declare one interface")
    }

    pub(super) fn one_func_interface(name: &str, func: &str, is_async: bool) -> ExportedInterface {
        ExportedInterface {
            name: Arc::from(name),
            wit: WitInterface::from(name),
            funcs: vec![ExportedFunc {
                name: Arc::from(func),
                param_tys: Arc::default(),
                result_tys: Arc::default(),
                is_async,
            }],
            resources: Vec::new(),
        }
    }

    /// Async-ness is part of a function's type identity, and introspection is
    /// where the host reads it. Getting it wrong lets a workload-facing import
    /// declared without `async` past its guard, to surface much later as an
    /// unhelpful "type mismatch with async".
    #[test]
    fn introspection_records_a_functions_declared_async_ness() {
        let wat = r#"
            (component
                (import "acme:kv/store@0.1.0" (instance
                    (export "get" (func (param "k" string)))
                ))
            )
        "#;
        let wasm = wat::parse_str(wat).expect("failed to parse WAT");
        let engine = Engine::builder().build().expect("failed to build engine");
        let component = Component::new(engine.inner(), &wasm).expect("failed to compile");

        let ifaces = introspect_imports(&component).expect("introspection should succeed");
        let store = ifaces
            .iter()
            .find(|i| &*i.name == "acme:kv/store@0.1.0")
            .expect("the imported interface should be introspected");
        assert!(
            !store.funcs[0].is_async,
            "a plain `func` must be recorded as sync, or the workload-facing guard lets it through"
        );
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
