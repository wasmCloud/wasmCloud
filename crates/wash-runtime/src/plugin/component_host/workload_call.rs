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
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use anyhow::Context as _;
use tracing::{debug, warn};
use wasmtime::AsContextMut;
use wasmtime::component::{Accessor, GuestTaskId, Linker, Resource, ResourceType, Val};

use crate::engine::ctx::SharedCtx;
use crate::engine::linked_call::{LinkedExportInvocation, invoke_linked_async_export};
use crate::engine::workload::{ExternalCallFunc, ResolvedWorkload};

use super::{ComponentHostPluginState, ExportedInterface, caller_root_task};

/// Interface name of the host workload import a plugin uses to name the
/// workload its calls go to. Versioned at the release that introduced the
/// interface: wasmtime resolves a plugin's import of a later, semver-compatible
/// `wasmcloud:host` against this definition, so the constant does not move when
/// the package version does.
pub(super) const HOST_WORKLOAD_INTERFACE: &str = "wasmcloud:host/workload@0.1.2";

/// A live `target` handle: the workload it names and the guest task it was
/// constructed on. The task is recorded here rather than read from the call
/// stack at destruction, because a handle may be dropped from a context that no
/// longer has the constructing task on its stack (store teardown, most of all).
struct TargetHandle {
    workload_id: Arc<str>,
    task: Option<GuestTaskId>,
}

/// The one component of a workload serving an interface this plugin imports,
/// with a resolved invocation per function of that interface.
struct InterfaceRoute {
    /// Manifest name of the serving component. The tie-break when several
    /// components of one workload export the same interface — the host
    /// dispatches to one, and picking by name keeps that choice stable across
    /// deploys, where component ids (fresh UUIDs) would not.
    component_name: Arc<str>,
    funcs: BTreeMap<Arc<str>, Arc<LinkedExportInvocation>>,
}

/// Every interface of one workload this plugin can call, keyed by the plugin's
/// own import instance name.
type WorkloadRoutes = BTreeMap<Arc<str>, InterfaceRoute>;

/// The `target` handles live on one guest task, innermost last. Each is paired
/// with the resource-table rep that identifies it, so a handle dropped out of
/// order removes its own entry rather than whichever is on top.
type TargetStack = Vec<(u32, Arc<str>)>;

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
    routes: RwLock<BTreeMap<Arc<str>, WorkloadRoutes>>,
    /// Guest task → its stack of live `target` handles, innermost last. A stack
    /// rather than a single slot so handles nest: dropping one restores the
    /// workload the handle it shadowed names.
    targets: Mutex<BTreeMap<Option<GuestTaskId>, TargetStack>>,
}

impl WorkloadCalls {
    pub(super) fn new(plugin_id: &'static str, imports: Vec<ExportedInterface>) -> Self {
        Self {
            plugin_id,
            imports,
            routes: RwLock::new(BTreeMap::new()),
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

    /// Claim `component_id` as the component serving whichever of this plugin's
    /// workload-facing imports it exports, resolving an invocation per function
    /// up front so a call is a map lookup rather than an export search.
    ///
    /// A component that exports none of them is simply not claimed — it bound
    /// to this plugin for a capability it imports instead.
    pub(super) async fn register(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        for import in &self.imports {
            let Some(component_name) = workload
                .component_exporting(component_id, &import.wit)
                .await
            else {
                continue;
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
            let invocations = workload
                .external_export_invocations(component_id, &import.name, &funcs)
                .await
                .with_context(|| {
                    format!(
                        "component '{component_id}' cannot serve {} for host component plugin '{}'",
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
        let workload_routes = routes.entry(Arc::from(workload_id)).or_default();
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

    /// Every workload this plugin can currently call, sorted by id.
    fn callable(&self) -> Vec<String> {
        self.routes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .map(|id| id.to_string())
            .collect()
    }

    /// The invocation for `interface`'s `func` on `workload_id`, if that
    /// workload is running and exports it.
    fn route(
        &self,
        workload_id: &str,
        interface: &str,
        func: &str,
    ) -> Option<Arc<LinkedExportInvocation>> {
        self.routes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(workload_id)?
            .get(interface)?
            .funcs
            .get(func)
            .map(Arc::clone)
    }

    /// Push a newly constructed `target` handle onto its task's stack.
    fn push_target(&self, task: Option<GuestTaskId>, rep: u32, workload_id: Arc<str>) {
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(task)
            .or_default()
            .push((rep, workload_id));
    }

    /// Remove a dropped handle from its task's stack. Removes by `rep` rather
    /// than popping, so handles dropped out of order still each remove their
    /// own entry; the task's entry goes when its last handle does.
    fn pop_target(&self, task: Option<GuestTaskId>, rep: u32) {
        let mut targets = self.targets.lock().unwrap_or_else(PoisonError::into_inner);
        let Entry::Occupied(mut stack) = targets.entry(task) else {
            return;
        };
        stack.get_mut().retain(|(held, _)| *held != rep);
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

    /// The workload the innermost live handle on `task` names. A `task` of
    /// `None` (the caller's async call stack was unavailable) never holds one:
    /// the constructor refuses to mint a handle it could not scope.
    fn current_target(&self, task: Option<GuestTaskId>) -> Option<Arc<str>> {
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&task)?
            .last()
            .map(|(_, workload_id)| Arc::clone(workload_id))
    }
}

/// Install this plugin's shims for `iface` — an interface it imports that a
/// workload exports — on the plugin's own linker. Where
/// [`super::add_capabilities_to_linker`] routes a *workload's* call into the
/// plugin store, these route the plugin's call out to a workload's store.
pub(super) fn add_workload_calls_to_linker(
    linker: &mut Linker<SharedCtx>,
    state: &Arc<ComponentHostPluginState>,
    iface: &ExportedInterface,
) -> anyhow::Result<()> {
    let mut linker_instance = linker
        .instance(&iface.name)
        .map_err(|e| e.context(format!("failed to open linker instance {}", iface.name)))?;

    for func in &iface.funcs {
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
                        route_workload_call(accessor, &state, &interface, &func, params, results)
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
async fn route_workload_call(
    accessor: &Accessor<SharedCtx>,
    state: &ComponentHostPluginState,
    interface: &str,
    func: &str,
    params: &[Val],
    results: &mut [Val],
) -> wasmtime::Result<()> {
    let task = accessor.with(|mut access| {
        let mut store = access.as_context_mut();
        caller_root_task(&mut store)
    });

    let target = match state.workload_calls.current_target(task) {
        Some(target) => target,
        None => {
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
            caller.workload_id
        }
    };

    let invocation = state
        .workload_calls
        .route(&target, interface, func)
        .ok_or_else(|| {
            wasmtime::format_err!(
                "host component plugin '{}' cannot call {interface}#{func} on workload \
                 '{target}': it is not running, or no component of it exports {interface}",
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

    let new_state = Arc::clone(state);
    instance
        .func_new(
            "[constructor]target",
            move |mut store, _ty, params, results| {
                let workload_id = match params.first() {
                    Some(Val::String(id)) => Arc::<str>::from(id.as_str()),
                    _ => wasmtime::bail!("target constructor expects a single string workload id"),
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
                let handle = store.data_mut().table.push(TargetHandle {
                    workload_id: Arc::clone(&workload_id),
                    task: Some(task),
                })?;
                let rep = handle.rep();
                let any = handle.try_into_resource_any(store.as_context_mut())?;
                // Recorded only once the handle exists in the table, so a
                // failure above cannot leave a target routing calls with
                // nothing left to ever drop it.
                new_state
                    .workload_calls
                    .push_target(Some(task), rep, workload_id);
                if let Some(slot) = results.first_mut() {
                    *slot = Val::Resource(any);
                }
                Ok(())
            },
        )
        .map_err(|e| e.context("failed to define wasmcloud:host/workload#[constructor]target"))?;

    instance
        .func_new(
            "[method]target.id",
            move |mut store, _ty, params, results| {
                let Some(Val::Resource(any)) = params.first() else {
                    wasmtime::bail!("target.id expects a target handle");
                };
                let handle = any.try_into_resource::<TargetHandle>(store.as_context_mut())?;
                let workload_id = store.data().table.get(&handle)?.workload_id.to_string();
                if let Some(slot) = results.first_mut() {
                    *slot = Val::String(workload_id);
                }
                Ok(())
            },
        )
        .map_err(|e| e.context("failed to define wasmcloud:host/workload#[method]target.id"))?;

    let callable_state = Arc::clone(state);
    instance
        .func_new("callable", move |_store, _ty, _params, results| {
            let callable = callable_state.workload_calls.callable();
            if let Some(slot) = results.first_mut() {
                *slot = Val::List(callable.into_iter().map(Val::String).collect());
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
        assert!(
            calls.current_target(None).is_none(),
            "no handle held means no target"
        );

        calls.push_target(None, 1, Arc::from("wl-outer"));
        calls.push_target(None, 2, Arc::from("wl-inner"));
        assert_eq!(
            calls.current_target(None).as_deref(),
            Some("wl-inner"),
            "the innermost handle wins"
        );

        calls.pop_target(None, 2);
        assert_eq!(
            calls.current_target(None).as_deref(),
            Some("wl-outer"),
            "dropping the inner handle restores the one it shadowed"
        );

        calls.push_target(None, 3, Arc::from("wl-middle"));
        calls.push_target(None, 4, Arc::from("wl-last"));
        calls.pop_target(None, 3);
        assert_eq!(
            calls.current_target(None).as_deref(),
            Some("wl-last"),
            "dropping a shadowed handle should not disturb the innermost one"
        );

        calls.pop_target(None, 4);
        calls.pop_target(None, 1);
        assert!(
            calls.current_target(None).is_none(),
            "dropping every handle leaves no target"
        );
    }

    /// A plugin with no workload-facing imports never routes a call this way,
    /// and reports nothing callable.
    #[test]
    fn a_plugin_that_only_serves_has_nothing_to_call() {
        let calls = WorkloadCalls::new("test-plugin", Vec::new());
        assert!(calls.is_empty());
        assert!(calls.callable().is_empty());
        assert!(
            calls
                .route("wl-a", "acme:events/handler@0.1.0", "notify")
                .is_none()
        );
    }
}
