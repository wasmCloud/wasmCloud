//! The plugin → workload direction: a host component plugin *importing* an
//! interface a workload exports, and calling into it.
//!
//! A host-native (Rust) plugin does this by holding a value. It learns at bind
//! time which components export the interface it wants to call (`wasi:http`'s
//! `exports_wasi_http`, messaging's `exports_messaging_handler`), captures
//! `(ResolvedWorkload, InstancePre, component_id)` in
//! [`HostPlugin::on_workload_resolved`], and invokes the export off whatever
//! event it likes — a request, a broker message, a timer.
//!
//! A plugin that is itself a component cannot hold that value: its import is
//! one static linker instance with one implementation and no parameter naming a
//! target. [`WorkloadCalls`] is that value, kept host-side on the plugin's
//! behalf, with two ways for the guest to say which entry it means:
//!
//! - **implicitly**, when the plugin is serving a capability call — the calling
//!   workload is already known from the caller's root guest task (the same
//!   resolution [`wasmcloud:host/identity`] uses), so a plugin calling back
//!   into its own caller addresses nothing; and
//! - **explicitly**, through a `wasmcloud:host/workload` `target` handle whose
//!   *lifetime* is the routing scope, which is what lets a plugin's own
//!   `wasi:cli/run` dispatch to a workload with no inbound call to inherit.
//!
//! The call itself reuses the workload↔workload machinery unchanged: each
//! function resolves to a [`LinkedExportInvocation`] pinned to the ephemeral
//! path, so a call runs on one of the callee's warm instances (or a store built
//! for it alone) and its arguments and results cross the boundary through
//! [`crate::engine::store::relocate`]. A `resource` handle cannot cross this
//! way and is rejected when the workload deploys.
//!
//! [`HostPlugin::on_workload_resolved`]: crate::plugin::HostPlugin::on_workload_resolved
//! [`wasmcloud:host/identity`]: super::install_host_identity

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use anyhow::Context as _;
use tracing::{debug, warn};
use wasmtime::AsContextMut;
use wasmtime::component::{Accessor, GuestTaskId, Linker, Resource, ResourceType, Val};

use crate::engine::ctx::SharedCtx;
use crate::engine::linked_call::{LinkedExportInvocation, invoke_linked_async_export};
use crate::engine::workload::{ExternalCallFunc, ItemExport, ResolvedWorkload};

use super::{ComponentHostPluginState, ExportedInterface, caller_root_task};

/// Interface name of the host workload import a plugin uses to name the
/// workload its calls go to. Versioned at the release that introduced the
/// interface: wasmtime resolves a plugin's import of a later, semver-compatible
/// `wasmcloud:host` against this definition, so the constant does not move when
/// the package version does.
pub(super) const HOST_WORKLOAD_INTERFACE: &str = "wasmcloud:host/workload@0.1.2";

/// A live `target` handle, as the guest holds it: the workload it names and the
/// task it was opened on. The routes it directs calls to live on that task's
/// [`TargetFrame`], which is where routing reads them.
///
/// The task is recorded here rather than read from the call stack at
/// destruction, because a handle may be dropped from a context that no longer
/// has the opening task on its stack (store teardown, most of all).
struct TargetHandle {
    workload_id: Arc<str>,
    task: Option<GuestTaskId>,
}

/// The one component of a workload serving an interface this plugin imports,
/// with a resolved invocation per function of that interface.
#[derive(Clone)]
struct InterfaceRoute {
    /// Manifest name of the serving component. The tie-break when several
    /// components of one workload export the same interface — the host
    /// dispatches to one, and picking by name keeps that choice stable across
    /// deploys, where component ids (fresh UUIDs) would not.
    component_name: Arc<str>,
    funcs: BTreeMap<Arc<str>, Arc<LinkedExportInvocation>>,
}

/// Every interface of one workload this plugin can call, keyed by the plugin's
/// own import instance name. Behind an `Arc` so an open `target` handle can
/// hold the set it resolved against.
type WorkloadRoutes = BTreeMap<Arc<str>, InterfaceRoute>;

/// Distinguishes one deployment of a workload id from the next to claim it.
/// Minted when a workload's first route is claimed and never reused.
type Generation = u64;

/// One deployment of a workload, as this plugin can call it.
struct CallableWorkload {
    /// Which deployment these routes belong to. A `target` handle records the
    /// generation it resolved against, so a handle held across a stop and a
    /// redeploy under the same id is refused rather than dispatched into the
    /// deployment it captured — the routes it holds name components of a
    /// workload that is gone.
    generation: Generation,
    routes: Arc<WorkloadRoutes>,
}

/// One live `target` handle on a task's stack: the resource-table rep that
/// identifies it (so a handle dropped out of order removes its own entry rather
/// than whichever is on top), and what it routes to.
///
/// Holding the routes pins *which* component serves each interface for this
/// handle's lifetime, so a second exporter appearing cannot move a call
/// mid-dispatch. It deliberately does not pin liveness: the `generation` beside
/// it is rechecked on every call, so a handle that outlived the deployment it
/// resolved against is refused rather than sent into components that are gone.
struct TargetFrame {
    rep: u32,
    workload_id: Arc<str>,
    generation: Generation,
    routes: Arc<WorkloadRoutes>,
    /// The registry job the opening call was running under, or `None` outside
    /// one (the plugin's own `cli/run`, or a task it spawned).
    ///
    /// A guest task id is reused once its task ends, and a handle the guest
    /// never drops — one it stashed to reuse — leaves its frame behind under
    /// that id. Without this, an unrelated later call assigned the same id
    /// would inherit the stale target and be delivered to a workload it never
    /// named. Job ids are unique within an incarnation and never reused, so
    /// comparing them tells a leaked frame from a live one.
    job: Option<crate::host::job_registry::JobId>,
}

/// The `target` handles live on one guest task, innermost last.
type TargetStack = Vec<TargetFrame>;

/// The workload-facing half of a host component plugin: which interfaces it
/// imports that a workload is expected to export, which workloads currently
/// serve them, and which workload each in-flight guest task is addressing.
pub(super) struct WorkloadCalls {
    plugin_id: &'static str,
    /// The plugin's imports that no host built-in satisfies, so a workload
    /// export must. Empty for a plugin that only provides capabilities, which
    /// makes every operation here a no-op.
    imports: Vec<ExportedInterface>,
    /// Workload id → the interfaces of it this plugin can call. Written when a
    /// workload resolves and when it unbinds; read on every call, so the lock
    /// is only ever held long enough to clone one `Arc` out.
    routes: RwLock<BTreeMap<Arc<str>, CallableWorkload>>,
    /// Source of the [`Generation`] stamped on each deployment. Monotonic for
    /// the plugin's lifetime, so a generation names one deployment of a workload
    /// id and never a later one.
    generations: AtomicU64,
    /// Guest task → its stack of live `target` handles, innermost last. A stack
    /// rather than a single slot so handles nest: dropping one restores the
    /// workload the handle it shadowed names.
    ///
    /// The key is an `Option` only so the tests below can name a task: a
    /// `GuestTaskId` is minted by wasmtime and cannot be constructed here.
    /// Nothing ever stores a `None` key in production — `open` refuses to hand
    /// out a handle whose task it could not resolve — so a `None` lookup is a
    /// miss, which is the answer a caller with no resolvable task should get.
    targets: Mutex<BTreeMap<Option<GuestTaskId>, TargetStack>>,
}

impl WorkloadCalls {
    pub(super) fn new(plugin_id: &'static str, imports: Vec<ExportedInterface>) -> Self {
        Self {
            plugin_id,
            imports,
            routes: RwLock::new(BTreeMap::new()),
            generations: AtomicU64::new(0),
            targets: Mutex::new(BTreeMap::new()),
        }
    }

    /// Whether this plugin imports anything a workload must export. `false` for
    /// a pure capability plugin, which never routes a call this way.
    pub(super) fn is_empty(&self) -> bool {
        self.imports.is_empty()
    }

    /// The interfaces a workload may satisfy by exporting them.
    pub(super) fn imports(&self) -> &[ExportedInterface] {
        &self.imports
    }

    /// Claim `item_id` as the component serving whichever of this plugin's
    /// workload-facing imports it exports, resolving an invocation per function
    /// up front so a call is a map lookup rather than an export search.
    ///
    /// An item that exports none of them is simply not claimed — it bound to
    /// this plugin for a capability it imports instead.
    pub(super) async fn register(
        &self,
        workload: &ResolvedWorkload,
        item_id: &str,
    ) -> anyhow::Result<()> {
        for import in &self.imports {
            let (component_name, export_name) =
                match workload.item_exporting(item_id, &import.wit).await {
                    ItemExport::Component { name, export } => (name, export),
                    ItemExport::Service => {
                        // Loudly, not silently: the workload deploys and reports
                        // healthy either way, so without this the only symptom
                        // is the plugin never seeing it in `callable` and every
                        // call naming it failing, with nothing to connect that
                        // to the service.
                        warn!(
                            id = self.plugin_id,
                            workload_id = workload.id(),
                            service = item_id,
                            interface = %import.name,
                            "a workload's long-lived service exports an interface this plugin \
                             calls, but a service is not callable from a plugin yet; move the \
                             export to a component to make it reachable"
                        );
                        continue;
                    }
                    ItemExport::None => continue,
                };
            let funcs: Vec<ExternalCallFunc<'_>> = import
                .funcs
                .iter()
                .map(|func| ExternalCallFunc {
                    name: &func.name,
                    param_tys: &func.param_tys,
                    result_tys: &func.result_tys,
                })
                .collect();
            // Addressed by the component's own export name, which matching may
            // have accepted despite differing from the plugin's import name.
            let invocations = workload
                .external_export_invocations(item_id, &export_name, &funcs)
                .await
                .with_context(|| {
                    format!(
                        "component '{item_id}' cannot serve {} for host component plugin '{}'",
                        import.name, self.plugin_id
                    )
                })?;
            let route = InterfaceRoute {
                component_name,
                funcs: invocations
                    .into_iter()
                    .map(|(name, inv)| (name, Arc::new(inv)))
                    .collect(),
            };
            self.insert_route(workload.id(), Arc::clone(&import.name), route);
        }
        Ok(())
    }

    /// Record `route` as the way to reach `interface` on `workload_id`, keeping
    /// the earlier-named component when one is already claimed.
    fn insert_route(&self, workload_id: &str, interface: Arc<str>, route: InterfaceRoute) {
        let mut routes = self.routes.write().unwrap_or_else(PoisonError::into_inner);
        let entry = routes
            .entry(Arc::from(workload_id))
            .or_insert_with(|| CallableWorkload {
                generation: self.generations.fetch_add(1, Ordering::Relaxed),
                routes: Arc::default(),
            });
        // Copy-on-write: an already-open `target` handle keeps the snapshot it
        // resolved against rather than seeing this workload's routes change
        // under it mid-dispatch.
        let workload_routes = Arc::make_mut(&mut entry.routes);
        match workload_routes.entry(interface) {
            Entry::Vacant(slot) => {
                debug!(
                    id = self.plugin_id,
                    %workload_id,
                    interface = %slot.key(),
                    component = %route.component_name,
                    "host component plugin can call a workload export"
                );
                slot.insert(route);
            }
            Entry::Occupied(mut slot) => {
                // The host dispatches this interface to one component per
                // workload, so a second exporter is ignored — deterministically,
                // by name, mirroring how the HTTP entrypoint picks among several
                // components carrying the same export.
                let (selected, ignored) = if route.component_name < slot.get().component_name {
                    let ignored = Arc::clone(&slot.get().component_name);
                    let selected = Arc::clone(&route.component_name);
                    slot.insert(route);
                    (selected, ignored)
                } else {
                    (
                        Arc::clone(&slot.get().component_name),
                        Arc::clone(&route.component_name),
                    )
                };
                warn!(
                    id = self.plugin_id,
                    %workload_id,
                    interface = %slot.key(),
                    %selected,
                    %ignored,
                    "multiple components export an interface the plugin calls; routing to one and \
                     ignoring the rest"
                );
            }
        }
    }

    /// Forget every route into `workload_id` — it has stopped, so its stores
    /// and warm instances are going away with it.
    pub(super) fn unregister(&self, workload_id: &str) {
        self.routes
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(workload_id);
    }

    /// Every workload this plugin can currently call, each with the interfaces
    /// of it that are callable. Sorted by id, and each entry's interfaces sorted
    /// by name, both by `BTreeMap` iteration.
    ///
    /// The interfaces are per workload rather than per plugin because a plugin
    /// may import several and a workload need only export some: calling one it
    /// does not export has no error result to return, so the plugin has to be
    /// told which are safe rather than left to find out by trapping.
    fn callable(&self) -> Vec<(String, Vec<String>)> {
        self.routes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(id, callable)| {
                let interfaces = callable
                    .routes
                    .keys()
                    .map(|interface| interface.to_string())
                    .collect();
                (id.to_string(), interfaces)
            })
            .collect()
    }

    /// Whether the deployment of `workload_id` stamped `generation` is still the
    /// one this plugin may call. A held `target` handle keeps its route
    /// snapshot, so this is the only thing that notices the workload stopping
    /// underneath it — and comparing the generation rather than the id alone is
    /// what makes a redeploy under that id count as stopping too, since the
    /// snapshot names components of the deployment that has gone.
    fn is_current(&self, workload_id: &str, generation: Generation) -> bool {
        self.routes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(workload_id)
            .is_some_and(|callable| callable.generation == generation)
    }

    /// The routes into `workload_id`, with the generation they belong to, if it
    /// is currently callable. What `open` checks, and what the handle it returns
    /// then holds.
    fn routes_for(&self, workload_id: &str) -> Option<(Generation, Arc<WorkloadRoutes>)> {
        self.routes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(workload_id)
            .map(|callable| (callable.generation, Arc::clone(&callable.routes)))
    }

    /// Push a newly opened `target` handle onto its task's stack.
    fn push_target(&self, task: Option<GuestTaskId>, frame: TargetFrame) {
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(task)
            .or_default()
            .push(frame);
    }

    /// Remove a dropped handle from its task's stack. Removes by `rep` rather
    /// than popping, so handles dropped out of order still each remove their
    /// own entry; the task's entry goes when its last handle does.
    fn pop_target(&self, task: Option<GuestTaskId>, rep: u32) {
        let mut targets = self.targets.lock().unwrap_or_else(PoisonError::into_inner);
        let Entry::Occupied(mut stack) = targets.entry(task) else {
            return;
        };
        stack.get_mut().retain(|frame| frame.rep != rep);
        if stack.get().is_empty() {
            stack.remove();
        }
    }

    /// Forget every live handle, because the store holding them is gone. The
    /// supervisor calls this as it builds each incarnation: a faulted store's
    /// guests never ran their destructors, and guest task ids are reused once
    /// their task ends, so a stranded stack would answer for an unrelated task
    /// in the next incarnation.
    pub(super) fn clear_targets(&self) {
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// The workload the innermost live handle on `task` names, with the routes
    /// that handle holds. A `task` of `None` (the caller's async call stack was
    /// unavailable) never holds one: `open` refuses to hand out a handle it
    /// could not scope.
    ///
    /// `job` is the registry job the *reading* call runs under. A frame whose
    /// job differs was opened by an earlier call that happened to be assigned
    /// the same guest task id and never dropped its handle; it is not this
    /// call's target and is ignored.
    fn current_target(
        &self,
        task: Option<GuestTaskId>,
        job: Option<crate::host::job_registry::JobId>,
    ) -> Option<(Arc<str>, Generation, Arc<WorkloadRoutes>)> {
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&task)?
            .iter()
            .rev()
            .find(|frame| frame.job == job)
            .map(|frame| {
                (
                    Arc::clone(&frame.workload_id),
                    frame.generation,
                    Arc::clone(&frame.routes),
                )
            })
    }
}

/// The invocation for `interface`'s `func` within one workload's routes.
fn invocation_in(
    routes: &WorkloadRoutes,
    interface: &str,
    func: &str,
) -> Option<Arc<LinkedExportInvocation>> {
    routes.get(interface)?.funcs.get(func).map(Arc::clone)
}

/// Install this plugin's shims for `iface` — an interface it imports that a
/// workload exports — on the plugin's own linker. Where
/// [`super::add_capabilities_to_linker`] routes a *workload's* call into the
/// plugin store, these route the plugin's call out to a workload's store.
///
/// Every function has to be `async func`, because every shim the host installs
/// is concurrent and async-ness is part of a function's type identity: a plain
/// `func` would fail to link with "type mismatch with async". Refused here, with
/// a message that names the fix, rather than surfacing as that error much later
/// at the plugin's own instantiation.
pub(super) fn add_workload_calls_to_linker(
    linker: &mut Linker<SharedCtx>,
    state: &Arc<ComponentHostPluginState>,
    iface: &ExportedInterface,
) -> anyhow::Result<()> {
    let mut linker_instance = linker
        .instance(&iface.name)
        .map_err(|e| e.context(format!("failed to open linker instance {}", iface.name)))?;

    for func in &iface.funcs {
        anyhow::ensure!(
            func.is_async,
            "host component plugin imports {}/{} for a workload to export, but it is declared \
             without `async`. The host serves it with a concurrent shim, which a sync-typed \
             import cannot bind to. Declare the interface's functions `async func`.",
            iface.name,
            func.name,
        );
        let state = Arc::clone(state);
        let interface = Arc::clone(&iface.name);
        let func_name = Arc::clone(&func.name);
        linker_instance
            .func_new_concurrent(
                func.name.as_ref(),
                move |accessor, _func_ty, params: &[Val], results: &mut [Val]| {
                    let state = Arc::clone(&state);
                    let interface = Arc::clone(&interface);
                    let func = Arc::clone(&func_name);
                    Box::pin(async move {
                        route_workload_call(accessor, &state, interface, func, params, results)
                            .await
                    })
                },
            )
            .map_err(|e| {
                e.context(format!(
                    "failed to install workload-call shim for {}/{}",
                    iface.name, func.name
                ))
            })?;
    }
    Ok(())
}

/// Route one call from the plugin store out to the workload component that
/// exports it.
///
/// The target is the workload named by the innermost live `target` handle on
/// this task, or — with none held — the workload whose capability call this
/// task is serving. Both are resolved from the caller's root guest task, so
/// they stay exact while calls from other workloads interleave on the same
/// store.
///
/// A handle carries the routes it resolved when it opened, so *which* component
/// serves the interface cannot change under the call. Whether that deployment is
/// still running is a separate question, rechecked here rather than taken from
/// the snapshot: a handle held across a stop would otherwise keep instantiating
/// the stopped workload's components, running its guest code after the host
/// reported it gone. The recheck is by generation, not by workload id, so a
/// redeploy under the same id does not make a handle from the previous
/// deployment look live again — its routes still name the components that went
/// away with it.
///
/// Every failure here surfaces as a trap, because a workload-exported import
/// has no error result the host could return instead — which is why a plugin
/// checks with `target.open` rather than calling blind.
async fn route_workload_call(
    accessor: &Accessor<SharedCtx>,
    state: &ComponentHostPluginState,
    interface: Arc<str>,
    func: Arc<str>,
    params: &[Val],
    results: &mut [Val],
) -> wasmtime::Result<()> {
    let task = accessor.with(|mut access| {
        let mut store = access.as_context_mut();
        caller_root_task(&mut store)
    });
    let job = task.and_then(|task| state.registry()?.job_for_task(task));

    let (target, generation, routes) = match state.workload_calls.current_target(task, job) {
        Some(held) => held,
        None => {
            // No handle: this call belongs to whichever workload's capability
            // call the task is serving. That workload is by definition running
            // (it is mid-call), so its routes are looked up live.
            let caller = task
                .and_then(|task| state.registry()?.caller_for_task(task))
                .ok_or_else(|| {
                    wasmtime::format_err!(
                        "host component plugin '{}' called {interface}#{func} with no workload to \
                         call: it is not serving a workload's capability call, and holds no \
                         wasmcloud:host/workload target handle naming one",
                        state.id
                    )
                })?;
            let (generation, routes) = state
                .workload_calls
                .routes_for(&caller.workload_id)
                .ok_or_else(|| {
                    // A lifecycle hook carries no component id: the host is
                    // telling the plugin *about* a workload rather than running
                    // one of that workload's calls. Worth saying so, because
                    // the hook is the one context where the caller is a
                    // workload that is deliberately not callable, and the
                    // generic message would read as a misconfiguration.
                    if caller.component_id.is_none() {
                        wasmtime::format_err!(
                            "host component plugin '{}' cannot call {interface}#{func} on workload \
                             '{}' from a workload-lifecycle hook: a workload is callable only \
                             while it is running, and a hook is delivered either side of that — \
                             still deploying at on-workload-bind, already torn down at \
                             on-workload-unbind. Reclaim per-workload state by id instead.",
                            state.id,
                            caller.workload_id
                        )
                    } else {
                        wasmtime::format_err!(
                            "host component plugin '{}' cannot call {interface}#{func} back on its \
                             caller '{}': it is not running, or nothing in it exports {interface}",
                            state.id,
                            caller.workload_id
                        )
                    }
                })?;
            (caller.workload_id, generation, routes)
        }
    };

    // Liveness is a separate question from routing, and only routing is
    // snapshotted: a deployment that stopped since the handle opened must not be
    // instantiated again just because the handle still remembers how.
    if !state.workload_calls.is_current(&target, generation) {
        return Err(wasmtime::format_err!(
            "host component plugin '{}' cannot call {interface}#{func} on workload '{target}': the \
             deployment this call names is no longer running",
            state.id
        ));
    }

    let invocation = invocation_in(&routes, &interface, &func).ok_or_else(|| {
        wasmtime::format_err!(
            "host component plugin '{}' cannot call {interface}#{func} on workload '{target}': no \
             component of it exports {interface}#{func}",
            state.id
        )
    })?;

    invoke_linked_async_export(accessor, params, results, &invocation).await
}

/// Install the `wasmcloud:host/workload` import on the plugin's own linker: the
/// `target` handle that names which workload this task's calls go to, and
/// `callable` for a plugin's own background work to discover what it may
/// dispatch to. A plugin that does not import the interface leaves these
/// definitions unused.
pub(super) fn install_host_workload(
    linker: &mut Linker<SharedCtx>,
    state: &Arc<ComponentHostPluginState>,
) -> anyhow::Result<()> {
    let mut instance = linker
        .instance(HOST_WORKLOAD_INTERFACE)
        .map_err(|e| e.context("failed to open the host workload linker instance"))?;

    let drop_state = Arc::clone(state);
    instance
        .resource(
            "target",
            ResourceType::host::<TargetHandle>(),
            move |mut store, rep| {
                // A handle whose table entry is already gone (store teardown
                // racing the guest's own drop) has nothing left to unwind, and
                // failing here would trap a guest that did nothing wrong.
                match store
                    .data_mut()
                    .table
                    .delete(Resource::<TargetHandle>::new_own(rep))
                {
                    Ok(handle) => drop_state.workload_calls.pop_target(handle.task, rep),
                    Err(e) => {
                        debug!(err = %e, "dropped a workload target handle that was already gone");
                    }
                }
                Ok(())
            },
        )
        .map_err(|e| e.context("failed to register wasmcloud:host/workload target"))?;

    let open_state = Arc::clone(state);
    instance
        .func_new(
            "[static]target.open",
            move |mut store, _ty, params, results| {
                let workload_id = match params.first() {
                    Some(Val::String(id)) => Arc::<str>::from(id.as_str()),
                    _ => wasmtime::bail!("target.open expects a single string workload id"),
                };
                // A handle scopes routing to one guest task, so a caller whose
                // task cannot be resolved gets no handle at all: minting one
                // anyway would pool it with every other unresolvable caller and
                // could send their calls to a workload they never named.
                let Some(task) = caller_root_task(&mut store) else {
                    wasmtime::bail!(
                        "cannot name a workload target from here: the calling task is unknown, so \
                         the handle would have no call to scope"
                    );
                };
                // Resolving here rather than on the call is the whole point:
                // the handle holds this snapshot, so `none` is the plugin's one
                // chance to notice a workload it can no longer reach, and
                // *which* component serves the interface cannot move under a
                // call the handle scopes.
                let Some((generation, routes)) = open_state.workload_calls.routes_for(&workload_id)
                else {
                    if let Some(slot) = results.first_mut() {
                        *slot = Val::Option(None);
                    }
                    return Ok(());
                };
                let handle = store.data_mut().table.push(TargetHandle {
                    workload_id: Arc::clone(&workload_id),
                    task: Some(task),
                })?;
                let rep = handle.rep();
                let any = handle.try_into_resource_any(store.as_context_mut())?;
                // Recorded only once the handle exists in the table, so a
                // failure above cannot leave a target routing calls with
                // nothing left to ever drop it.
                let job = open_state
                    .registry()
                    .and_then(|registry| registry.job_for_task(task));
                open_state.workload_calls.push_target(
                    Some(task),
                    TargetFrame {
                        rep,
                        workload_id,
                        generation,
                        routes,
                        job,
                    },
                );
                if let Some(slot) = results.first_mut() {
                    *slot = Val::Option(Some(Box::new(Val::Resource(any))));
                }
                Ok(())
            },
        )
        .map_err(|e| e.context("failed to define wasmcloud:host/workload#[static]target.open"))?;

    instance
        .func_new(
            "[method]target.get-workload-id",
            move |mut store, _ty, params, results| {
                let Some(Val::Resource(any)) = params.first() else {
                    wasmtime::bail!("target.get-workload-id expects a target handle");
                };
                let handle = any.try_into_resource::<TargetHandle>(store.as_context_mut())?;
                let workload_id = store.data().table.get(&handle)?.workload_id.to_string();
                if let Some(slot) = results.first_mut() {
                    *slot = Val::String(workload_id);
                }
                Ok(())
            },
        )
        .map_err(|e| {
            e.context("failed to define wasmcloud:host/workload#[method]target.get-workload-id")
        })?;

    let callable_state = Arc::clone(state);
    instance
        .func_new("callable", move |_store, _ty, _params, results| {
            let callable = callable_state.workload_calls.callable();
            if let Some(slot) = results.first_mut() {
                *slot = Val::Map(
                    callable
                        .into_iter()
                        .map(|(workload_id, interfaces)| {
                            let interfaces =
                                interfaces.into_iter().map(Val::String).collect::<Vec<_>>();
                            (Val::String(workload_id), Val::List(interfaces))
                        })
                        .collect(),
                );
            }
            Ok(())
        })
        .map_err(|e| e.context("failed to define wasmcloud:host/workload#callable"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Handles nest: the innermost names the target, dropping it restores the
    /// one it shadowed, and dropping the last leaves no target at all. Dropped
    /// out of order, each still removes its own entry rather than whichever is
    /// on top — a guest is free to drop them in any order.
    #[test]
    fn target_handles_nest_and_unwind() {
        let calls = WorkloadCalls::new("test-plugin", Vec::new());
        let frame = |rep: u32, id: &str, job: Option<u64>| TargetFrame {
            rep,
            workload_id: Arc::from(id),
            generation: 0,
            routes: Arc::new(WorkloadRoutes::new()),
            job,
        };
        let held = |calls: &WorkloadCalls, job: Option<u64>| {
            calls
                .current_target(None, job)
                .map(|(workload_id, ..)| workload_id.to_string())
        };
        assert!(
            held(&calls, Some(1)).is_none(),
            "no handle held means no target"
        );

        calls.push_target(None, frame(1, "wl-outer", Some(1)));
        calls.push_target(None, frame(2, "wl-inner", Some(1)));
        assert_eq!(
            held(&calls, Some(1)).as_deref(),
            Some("wl-inner"),
            "the innermost handle wins"
        );

        calls.pop_target(None, 2);
        assert_eq!(
            held(&calls, Some(1)).as_deref(),
            Some("wl-outer"),
            "dropping the inner handle restores the one it shadowed"
        );

        calls.push_target(None, frame(3, "wl-middle", Some(1)));
        calls.push_target(None, frame(4, "wl-last", Some(1)));
        calls.pop_target(None, 3);
        assert_eq!(
            held(&calls, Some(1)).as_deref(),
            Some("wl-last"),
            "dropping a shadowed handle should not disturb the innermost one"
        );

        calls.pop_target(None, 4);
        calls.pop_target(None, 1);
        assert!(
            held(&calls, Some(1)).is_none(),
            "dropping every handle leaves no target"
        );
    }

    /// A handle the guest never drops outlives its task, and wasmtime reuses a
    /// guest task id once its task ends. The stale frame must not become the
    /// target of whatever call is assigned that id next — it belongs to a job
    /// that is over, and job ids are never reused.
    #[test]
    fn a_leaked_handle_is_not_inherited_by_a_later_call() {
        let calls = WorkloadCalls::new("test-plugin", Vec::new());
        // Job 1 opens a handle and never drops it; its task ends.
        calls.push_target(
            None,
            TargetFrame {
                rep: 1,
                workload_id: Arc::from("wl-other"),
                generation: 0,
                routes: Arc::new(WorkloadRoutes::new()),
                job: Some(1),
            },
        );

        // Job 2 is assigned the same task id and holds no handle of its own.
        assert!(
            calls.current_target(None, Some(2)).is_none(),
            "a later call must not inherit a leaked frame from an earlier job"
        );
        // The frame is still the target of its own job, which may still be
        // running — only the task id was reused.
        assert_eq!(
            calls
                .current_target(None, Some(1))
                .map(|(workload_id, ..)| workload_id.to_string())
                .as_deref(),
            Some("wl-other")
        );
    }

    /// A `target` handle names one *deployment*, not a workload id. Held across
    /// a stop and a redeploy under the same id, its routes still name components
    /// of the workload that went away, so the generation it captured has to read
    /// as gone even though the id is callable again.
    #[test]
    fn a_redeploy_under_the_same_id_does_not_revive_a_held_target() {
        let calls = WorkloadCalls::new("test-plugin", Vec::new());
        let claim = || {
            calls.insert_route(
                "wl-a",
                Arc::from("acme:events/handler@0.1.0"),
                InterfaceRoute {
                    component_name: Arc::from("events-caller"),
                    funcs: BTreeMap::new(),
                },
            );
        };

        claim();
        let (held, _routes) = calls.routes_for("wl-a").expect("the workload is callable");
        assert!(calls.is_current("wl-a", held));

        calls.unregister("wl-a");
        assert!(
            !calls.is_current("wl-a", held),
            "a stopped workload is not callable"
        );

        claim();
        assert!(
            calls.routes_for("wl-a").is_some(),
            "the new deployment is callable"
        );
        assert!(
            !calls.is_current("wl-a", held),
            "a handle opened against the previous deployment must not reach the new one"
        );
    }

    /// A workload-facing import has to be `async func`: the host serves it with
    /// a concurrent shim, which a sync-typed import cannot bind to. Refused at
    /// load, where the message can name the fix, rather than surfacing as a
    /// "type mismatch with async" at the plugin's own instantiation.
    #[test]
    fn a_sync_workload_facing_import_is_refused() {
        use super::super::tests::{idle_state, one_func_interface};
        use crate::engine::Engine;

        let engine = Engine::builder().build().expect("failed to build engine");
        let mut linker = Linker::new(engine.inner());
        let state = idle_state("sync-workload-call-plugin");
        let iface = one_func_interface("acme:events/handler@0.1.0", "notify", false);

        let err = add_workload_calls_to_linker(&mut linker, &state, &iface)
            .expect_err("a sync workload-facing import should be refused");
        let msg = err.to_string();
        assert!(
            msg.contains("concurrent shim") && msg.contains("async func"),
            "the error should name the hazard and the fix, got: {msg}"
        );
    }

    /// The same interface declared `async func` installs, which is what the
    /// `events-plugin` fixture and every real workload-facing import look like.
    #[test]
    fn an_async_workload_facing_import_installs() {
        use super::super::tests::{idle_state, one_func_interface};
        use crate::engine::Engine;

        let engine = Engine::builder().build().expect("failed to build engine");
        let mut linker = Linker::new(engine.inner());
        let state = idle_state("async-workload-call-plugin");
        let iface = one_func_interface("acme:events/handler@0.1.0", "notify", true);

        add_workload_calls_to_linker(&mut linker, &state, &iface)
            .expect("an async workload-facing import should install");
    }

    /// A plugin with no workload-facing imports never routes a call this way,
    /// and reports nothing callable.
    #[test]
    fn a_plugin_that_only_serves_has_nothing_to_call() {
        let calls = WorkloadCalls::new("test-plugin", Vec::new());
        assert!(calls.is_empty());
        assert!(calls.callable().is_empty());
        assert!(
            calls.routes_for("wl-a").is_none(),
            "an unknown workload is not callable, so `open` hands out no handle"
        );
    }
}
