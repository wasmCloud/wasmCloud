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

use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use tracing::{debug, error, warn};
use wasmtime::AsContextMut;
use wasmtime::Store;
use wasmtime::component::types::Type;
use wasmtime::component::{
    Accessor, Component, InstancePre, Linker, Resource, Val, types::ComponentItem,
};

use crate::engine::Engine;
use crate::engine::ctx::{CallerIdentity, Ctx, SharedCtx};
use crate::engine::store::relocate::{self, Relocated};
use crate::engine::store::resource_bridge::{self, ProxyResource};
use crate::engine::workload::WorkloadItem;
use crate::host::job_registry::JobRegistry;
use crate::host::trigger_service::{
    CapabilityCall, CapabilityFunc, CapabilityJob, Ingress, TriggerService,
};
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

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
}

impl ComponentHostPluginState {
    /// A clone of the current capability sender, or `None` if the plugin is not
    /// running (before `start()` / after `stop()` / restart budget exhausted).
    fn sender(&self) -> Option<CapabilitySender> {
        self.tx.load_full().map(|tx| (*tx).clone())
    }

    /// The current incarnation's job registry, or `None` if the plugin is not
    /// running. Read by the host `identity`/`cancel` imports.
    fn registry(&self) -> Option<Arc<JobRegistry>> {
        self.registry.load_full()
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
    pub fn new(id: &'static str, wasm: &[u8], engine: Engine) -> anyhow::Result<Self> {
        let state = Arc::new(ComponentHostPluginState {
            id,
            tx: ArcSwapOption::empty(),
            supervisor: Mutex::new(None),
            registry: ArcSwapOption::empty(),
        });

        let (exports, pre) = build_plugin_linker(&engine, id, wasm, &state)?;

        let world = WitWorld {
            imports: exports.iter().map(|e| e.wit.clone()).collect(),
            exports: Default::default(),
        };
        let capability_funcs = exports
            .iter()
            .flat_map(|e| {
                e.funcs.iter().map(|f| CapabilityFunc {
                    interface: Arc::clone(&e.name),
                    func: Arc::clone(&f.name),
                })
            })
            .collect();

        Ok(Self {
            id,
            engine,
            pre,
            world,
            exports: Arc::new(exports),
            capability_funcs,
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
}

#[async_trait]
impl HostPlugin for ComponentHostPlugin {
    fn id(&self) -> &'static str {
        self.id
    }

    fn world(&self) -> WitWorld {
        self.world.clone()
    }

    async fn start(&self) -> anyhow::Result<()> {
        let (tx, rx) = tokio::sync::mpsc::channel(CAPABILITY_CHANNEL_CAPACITY);
        self.state.tx.store(Some(Arc::new(tx)));

        let supervisor = tokio::spawn(run_supervisor(
            self.engine.clone(),
            self.pre.clone(),
            self.capability_funcs.clone(),
            Arc::clone(&self.state),
            self.max_restarts,
            rx,
        ));
        *self
            .state
            .supervisor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(supervisor);
        debug!(id = self.id, "started host component plugin");
        Ok(())
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
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

    async fn stop(&self) -> anyhow::Result<()> {
        // Clearing the sender closes the current incarnation's channel, ending
        // the TriggerService's serve loop and letting the supervisor exit cleanly; the
        // registry goes with it (the driver's tasks retire their jobs as they end).
        self.state.tx.store(None);
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

/// Build the plugin store's linker and pre-instantiate the component against it.
/// This is the single place that declares the plugin's whole import surface:
///
/// - the WASI (and `wasi:http`) base, from [`Engine::prepare_host_component`];
/// - the `wasmcloud:host/identity` import (unused unless the plugin imports it);
/// - a route back to the plugin's own capability channel for any interface it
///   both imports and exports (a self-import).
///
/// Returns the introspected exports alongside the resulting [`InstancePre`].
fn build_plugin_linker(
    engine: &Engine,
    id: &str,
    wasm: &[u8],
    state: &Arc<ComponentHostPluginState>,
) -> anyhow::Result<(Vec<ExportedInterface>, InstancePre<SharedCtx>)> {
    let (component, mut linker) = engine.prepare_host_component(wasm)?;
    let exports = introspect_exports(&component)?;
    anyhow::ensure!(
        exports.iter().any(|e| !e.funcs.is_empty()),
        "host component plugin '{id}' exports no capability functions to serve"
    );

    install_host_identity(&mut linker, state)
        .with_context(|| format!("failed to install host identity on plugin '{id}'"))?;
    install_host_cancel(&mut linker, state)
        .with_context(|| format!("failed to install host cancel on plugin '{id}'"))?;

    for imported in introspect_imports(&component)? {
        if exports.iter().any(|e| e.name == imported.name) {
            add_capabilities_to_linker(&mut linker, state, &imported).with_context(|| {
                format!(
                    "failed to wire self-import {} on plugin '{id}'",
                    imported.name
                )
            })?;
        }
    }

    linker
        .instantiate_pre(&component)
        .map_err(anyhow::Error::from)
        .context("failed to pre-instantiate host component plugin")
        .map(|pre| (exports, pre))
}

/// Interface name of the host identity import a plugin may use to partition
/// state by caller.
const HOST_IDENTITY_INTERFACE: &str = "wasmcloud:host/identity@0.1.0";

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
                let root = store.async_call_stack().ok().and_then(|stack| stack.last());
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
                let root = store.async_call_stack().ok().and_then(|stack| stack.last());
                let id = root
                    .and_then(|task| component_state.registry()?.caller_for_task(task))
                    .map(|c| c.component_id.to_string())
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
            let root = store.async_call_stack().ok().and_then(|stack| stack.last());
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
            let root = store.async_call_stack().ok().and_then(|stack| stack.last());
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
            let root = store.async_call_stack().ok().and_then(|stack| stack.last());
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
                    component_id: Arc::clone(&ctx.component_id),
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
async fn run_supervisor(
    engine: Engine,
    pre: InstancePre<SharedCtx>,
    funcs: Vec<CapabilityFunc>,
    state: Arc<ComponentHostPluginState>,
    max_restarts: u32,
    mut rx: tokio::sync::mpsc::Receiver<CapabilityJob>,
) {
    let mut restarts = 0u32;
    loop {
        let store = build_plugin_store(&engine, state.id);
        // A fresh job registry per incarnation, published on `state` so the
        // baked-in identity/cancel imports reach this store's live jobs. Stale
        // jobs from a faulted incarnation die with its store (their guards retire
        // as the tasks drop).
        let registry = JobRegistry::new();
        state.registry.store(Some(Arc::clone(&registry)));
        let ingress = Ingress::Capability {
            funcs: funcs.clone(),
            rx,
            registry,
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
        // budget — only rapid crash loops should exhaust it.
        if uptime >= crate::timeouts::plugin_healthy_uptime() {
            restarts = 0;
        }

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

        // Fresh channel for the new incarnation; installed shims read the new
        // sender via `state.sender()` on their next call, and calls made during
        // the backoff below queue on it rather than failing.
        let (new_tx, new_rx) = tokio::sync::mpsc::channel(CAPABILITY_CHANNEL_CAPACITY);
        state.tx.store(Some(Arc::new(new_tx)));
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
/// own id.
fn build_plugin_store(engine: &Engine, id: &'static str) -> Store<SharedCtx> {
    let ctx = Ctx::builder(id, id).build();
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

        interfaces.push(ExportedInterface {
            name: iface_name.into(),
            wit: WitInterface::from(iface_name),
            funcs,
            resources,
        });
    }

    Ok(interfaces)
}
