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
//! - **explicitly**, through a `wasmcloud:host/workload-call` `target` handle
//!   whose *lifetime* is the routing scope, which is what lets a plugin's own
//!   `wasi:cli/run` dispatch to a workload with no inbound call to inherit.
//!
//! `wasi:cli/run` is a workload-facing import like any other: a plugin
//! importing it runs the component of a workload that exports `run`. It is the
//! one interface of `wasi:cli` the WASI base does not serve, so it alone
//! escapes [`super::is_base_wasi`] — see [`super::is_cli_run`].
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
use wasmtime::component::types::Type;
use wasmtime::component::{Accessor, GuestTaskId, Linker, Resource, ResourceType, Val};

use crate::engine::ctx::SharedCtx;
use crate::engine::linked_call::{LinkedExportInvocation, invoke_linked_async_export};
use crate::engine::workload::{ExternalCallFunc, ItemExport, ResolvedWorkload};

use super::{ComponentHostPluginState, ExportedInterface, caller_root_task};

/// Interface name of the host workload-call import a plugin uses to name the
/// workload its calls go to. Versioned at the release that introduced the
/// interface: wasmtime resolves a plugin's import of a later, semver-compatible
/// `wasmcloud:host` against this definition, so the constant does not move when
/// the package version does.
pub(super) const HOST_WORKLOAD_CALL_INTERFACE: &str = "wasmcloud:host/workload-call@0.1.2";

/// A live `target` handle, as the guest holds it: the workload it names and the
/// task it was opened on. The routes it directs calls to live on that task's
/// [`TargetFrame`], which is where routing reads them.
///
/// The task is recorded here rather than read from the call stack at
/// destruction, because a handle may be dropped from a context that no longer
/// has the opening task on its stack (store teardown, most of all).
struct TargetHandle {
    workload_id: Arc<str>,
    interface: Arc<str>,
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
/// own import instance name. Each route is behind its own `Arc` so a handle —
/// and every call through one — takes a pointer to it rather than a copy of its
/// function map.
type WorkloadRoutes = BTreeMap<Arc<str>, Arc<InterfaceRoute>>;

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
/// A frame binds a workload *and* one of its interfaces, resolved together when
/// the handle opened. That pairing is what makes a held handle proof the call
/// can be routed: the route below exists, so there is no "does this workload
/// export it" left to fail on at call time.
///
/// Holding the route also pins *which* component serves the interface, so a
/// second exporter appearing cannot move the call mid-dispatch. It deliberately
/// does not pin liveness: the `generation` beside it is rechecked on every call,
/// so a handle that outlived the deployment it resolved against is refused
/// rather than sent into components that are gone.
struct TargetFrame {
    rep: u32,
    workload_id: Arc<str>,
    interface: Arc<str>,
    generation: Generation,
    route: Arc<InterfaceRoute>,
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
                    // Every service exports `wasi:cli/run` — that is what makes
                    // it a service — so warning would fire for each one a
                    // run-importing plugin binds, on the normal shape where a
                    // component of the same workload is the real target. It is
                    // still the whole story when no component exports `run`, so
                    // the case stays traceable rather than silent.
                    ItemExport::Service if import.wit.is_cli_run() => {
                        debug!(
                            id = self.plugin_id,
                            workload_id = workload.id(),
                            service = item_id,
                            "skipping a workload's service for wasi:cli/run; only a component's \
                             run export is callable from a plugin"
                        );
                        continue;
                    }
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
                slot.insert(Arc::new(route));
            }
            Entry::Occupied(mut slot) => {
                // The host dispatches this interface to one component per
                // workload, so a second exporter is ignored — deterministically,
                // by name, mirroring how the HTTP entrypoint picks among several
                // components carrying the same export.
                let (selected, ignored) = if route.component_name < slot.get().component_name {
                    let ignored = Arc::clone(&slot.get().component_name);
                    let selected = Arc::clone(&route.component_name);
                    slot.insert(Arc::new(route));
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
    /// is currently callable.
    fn routes_for(&self, workload_id: &str) -> Option<(Generation, Arc<WorkloadRoutes>)> {
        self.routes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(workload_id)
            .map(|callable| (callable.generation, Arc::clone(&callable.routes)))
    }

    /// The route to `interface` on `workload_id`, if that workload is running
    /// and serves it. What `open` resolves, and what the handle it returns then
    /// holds — so a handle that exists is a pairing the host has already agreed
    /// to, not a request to be checked later.
    fn route_for(
        &self,
        workload_id: &str,
        interface: &str,
    ) -> Option<(Generation, Arc<InterfaceRoute>)> {
        let routes = self.routes.read().unwrap_or_else(PoisonError::into_inner);
        let callable = routes.get(workload_id)?;
        let route = callable.routes.get(interface)?;
        Some((callable.generation, Arc::clone(route)))
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

    /// The workload the innermost live handle on `task` for `interface` names,
    /// with the route that handle holds. A handle opened for a different
    /// interface does not answer here — it redirects only what it was opened
    /// for, so anything else falls back to the caller. A `task` of `None` (the caller's async call stack was
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
        interface: &str,
    ) -> Option<(Arc<str>, Generation, Arc<InterfaceRoute>)> {
        self.targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&task)?
            .iter()
            .rev()
            .find(|frame| frame.job == job && &*frame.interface == interface)
            .map(|frame| {
                (
                    Arc::clone(&frame.workload_id),
                    frame.generation,
                    Arc::clone(&frame.route),
                )
            })
    }
}

/// Which failure a `call-error` case names, so the host picks one rather than
/// only ever saying "something went wrong".
#[derive(Clone, Copy)]
enum CallFailure {
    NoTarget,
    NotRunning,
    NotExported,
    Failed,
    /// A failure the host has no dedicated case for. Reached only by defensive
    /// branches whose condition the deploy-time checks already rule out, which
    /// is exactly what `other` is for — and keeps the case exercised rather
    /// than dead until the host grows a use for it.
    Other,
}

impl CallFailure {
    /// The `wasmcloud:host/types.call-error` case this failure is reported as.
    fn case(self) -> &'static str {
        match self {
            Self::NoTarget => "no-target",
            Self::NotRunning => "not-running",
            Self::NotExported => "not-exported",
            Self::Failed => "failed",
            Self::Other => "other",
        }
    }
}

/// How a workload-facing function is able to carry a failure back to the
/// plugin. Decided when the plugin loads, from the function's declared result,
/// and captured by its shim so the failure path never has to look it up.
#[derive(Clone)]
pub(super) enum ErrorShape {
    /// `result<_, string>` — the failure is flattened to its message. What an
    /// off-the-shelf interface like `wasmcloud:messaging/handler` already has.
    Message,
    /// `result<_, call-error>` — the recommended form. The failure keeps its
    /// case, so a plugin can tell "this workload is gone" from "this call
    /// trapped" without parsing, and tell either from an error the *workload*
    /// meant to return.
    Structured,
    /// The interface's own error type, with the one case the host picked to
    /// build. Lets an interface the plugin does not own — a `wasi:sockets`-style
    /// `error-code`, say — still be answerable.
    ///
    /// The cost is that the plugin cannot tell a host failure from one the
    /// workload meant, because both arrive as the same case. Where the case has
    /// somewhere to put text the detail is prefixed with
    /// [`HOST_FAILURE_PREFIX`] so it is at least sniffable; where it has not,
    /// the reason is simply lost. Prefer `call-error` when you own the
    /// interface.
    Borrowed(BorrowedCase),
    /// An error arm carrying no payload — `result<T>` or a bare `result` — so
    /// the plugin learns only that the call did not succeed. `wasi:cli/run` is
    /// shaped this way.
    Empty,
}

/// The case of an interface's own error type the host builds, and how.
#[derive(Clone)]
pub(super) struct BorrowedCase {
    case: Arc<str>,
    build: CaseBuild,
}

/// What a case needs to become a value.
#[derive(Clone, Copy)]
enum CaseBuild {
    /// An `enum` case: no payload anywhere in the type.
    Enum,
    /// A `variant` case carrying nothing.
    Unit,
    /// A `variant` case carrying `string`.
    Str,
    /// A `variant` case carrying `option<string>`.
    OptStr,
}

/// Marks a failure the *host* produced, for the shapes whose type cannot say so
/// on its own. A plugin matching on a borrowed case can look for this to tell
/// the two apart; `call-error` needs no such convention.
const HOST_FAILURE_PREFIX: &str = "wasmcloud:host: ";

/// Cases the host prefers when it has to pick one of an interface's own, in
/// order. Each names "something went wrong in a way this interface did not
/// enumerate", which is exactly what a host failure is — `wasi:http`'s
/// `internal-error` is the case in point. Anything constructible will do if none
/// of these are present, but a named one carries closer to the right meaning.
const PREFERRED_BORROWED_CASES: &[&str] = &["internal-error", "internal", "unknown", "other"];

/// Whether `result_tys` can carry a host failure, and in which form.
///
/// A workload-facing import must return exactly one `result` whose error arm the
/// host can build a value of, because the host answers a failed call with a
/// value rather than a trap. Four ways that holds, in descending order of how
/// much the plugin learns:
///
/// 1. the `call-error` variant, recognised structurally — by its case names and
///    their `option<string>` payloads — so a plugin may `use` the host's
///    definition or restate it and either links;
/// 2. a plain `string`;
/// 3. the interface's own error type, if any one of its cases is something the
///    host knows how to construct ([`borrowed_case`]); or
/// 4. an error arm with no payload, which says only that the call failed — the
///    shape `wasi:cli/run`'s `run: async func() -> result` has.
///
/// All four `result` forms the component model spells are accepted:
/// `result<T, E>` and `result<_, E>` through rungs 1-3 by their error type,
/// `result<T>` and a bare `result` through rung 4.
pub(super) fn error_shape(result_tys: &[Type]) -> Option<ErrorShape> {
    let [Type::Result(result)] = result_tys else {
        return None;
    };
    // `result<T>` and a bare `result` both have an error arm; it just carries
    // no payload. Whether an ok type is present says nothing about the host's
    // ability to report a failure.
    let Some(err) = result.err() else {
        return Some(ErrorShape::Empty);
    };
    match err {
        Type::String => Some(ErrorShape::Message),
        Type::Variant(variant) => {
            let cases: Vec<(&str, Option<Type>)> =
                variant.cases().map(|case| (case.name, case.ty)).collect();
            if is_call_error(&cases) {
                return Some(ErrorShape::Structured);
            }
            borrowed_case(cases.into_iter().map(|(name, ty)| {
                let build = match ty {
                    None => Some(CaseBuild::Unit),
                    Some(Type::String) => Some(CaseBuild::Str),
                    Some(Type::Option(inner)) if inner.ty() == Type::String => {
                        Some(CaseBuild::OptStr)
                    }
                    Some(_) => None,
                };
                (name, build)
            }))
        }
        // An `enum` is all payload-less cases, so every one is constructible and
        // the detail has nowhere to go.
        Type::Enum(names) => borrowed_case(names.names().map(|n| (n, Some(CaseBuild::Enum)))),
        _ => None,
    }
}

/// Whether these cases are `wasmcloud:host/types.call-error`.
fn is_call_error(cases: &[(&str, Option<Type>)]) -> bool {
    cases.len() == CALL_ERROR_CASES.len()
        && CALL_ERROR_CASES
            .iter()
            .all(|name| cases.iter().any(|(case, _)| case == name))
        && cases
            .iter()
            .all(|(_, ty)| matches!(ty, Some(Type::Option(inner)) if inner.ty() == Type::String))
}

/// Pick the case the host will build, preferring one whose name already means
/// "unenumerated failure" and otherwise taking the first it can construct in
/// declaration order. `None` when the type offers nothing constructible — every
/// case carries a record or a resource — which is where load refuses the import.
fn borrowed_case<'a>(
    cases: impl Iterator<Item = (&'a str, Option<CaseBuild>)>,
) -> Option<ErrorShape> {
    let constructible: Vec<(&str, CaseBuild)> = cases
        .filter_map(|(name, build)| build.map(|build| (name, build)))
        .collect();
    let chosen = PREFERRED_BORROWED_CASES
        .iter()
        .find_map(|preferred| {
            constructible
                .iter()
                .find(|(name, _)| name == preferred)
                .copied()
        })
        .or_else(|| constructible.first().copied())?;
    Some(ErrorShape::Borrowed(BorrowedCase {
        case: Arc::from(chosen.0),
        build: chosen.1,
    }))
}

/// Every case of `wasmcloud:host/types.call-error`, which is what
/// [`error_shape`] matches a plugin's declared error type against. `other` is
/// among them: a plugin has to be able to receive it.
const CALL_ERROR_CASES: &[&str] = &[
    "no-target",
    "not-running",
    "not-exported",
    "failed",
    "other",
];

/// Write `failure` into the call's result slot, so the plugin receives an error
/// value rather than a trap.
///
/// This is the whole of the no-trap contract: a workload-facing call reaches
/// another workload's guest, which can trap, stop, or run long, and none of that
/// is the plugin's fault — so none of it faults the store the plugin shares with
/// every other workload it serves.
fn report_failure(
    results: &mut [Val],
    shape: &ErrorShape,
    failure: CallFailure,
    detail: String,
) -> wasmtime::Result<()> {
    let err = match shape {
        // Nowhere to put the reason: every other shape hands the plugin the
        // detail, so this log is the only record. A call that could not be
        // routed at all is a deploy the operator can fix, and the plugin cannot
        // tell it from a run that genuinely failed — so say so loudly, and
        // leave the failures the callee itself produced at debug.
        ErrorShape::Empty => {
            match failure {
                CallFailure::NoTarget | CallFailure::NotExported | CallFailure::NotRunning => {
                    warn!(
                        %detail,
                        case = failure.case(),
                        "workload-facing call could not be routed, and its signature has no room \
                         to say why"
                    );
                }
                CallFailure::Failed | CallFailure::Other => {
                    debug!(%detail, case = failure.case(), "workload-facing call failed");
                }
            }
            None
        }
        ErrorShape::Message => Some(Val::String(format!("{HOST_FAILURE_PREFIX}{detail}"))),
        ErrorShape::Structured => Some(Val::Variant(
            failure.case().to_string(),
            Some(Box::new(Val::Option(Some(Box::new(Val::String(detail)))))),
        )),
        ErrorShape::Borrowed(BorrowedCase { case, build }) => Some({
            let case = case.to_string();
            // The prefix is built only for a case with room for it; the others
            // drop the reason entirely, which is the cost of borrowing a type
            // that never meant to carry one.
            let detail = || format!("{HOST_FAILURE_PREFIX}{detail}");
            match build {
                CaseBuild::Enum => Val::Enum(case),
                CaseBuild::Unit => Val::Variant(case, None),
                CaseBuild::Str => Val::Variant(case, Some(Box::new(Val::String(detail())))),
                CaseBuild::OptStr => Val::Variant(
                    case,
                    Some(Box::new(Val::Option(Some(Box::new(Val::String(detail())))))),
                ),
            }
        }),
    };
    let slot = results.first_mut().ok_or_else(|| {
        wasmtime::format_err!("workload-facing call has no result slot to report a failure through")
    })?;
    *slot = Val::Result(Err(err.map(Box::new)));
    Ok(())
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
        // Decided here and captured, so the failure path never has to work out
        // how to say so. Load refuses a function without one
        // ([`super::classify_workload_imports`]), which is what makes every exit
        // below a value rather than a trap.
        let shape = error_shape(&func.result_tys).with_context(|| {
            format!(
                "workload-facing import {}/{} cannot carry a failure",
                iface.name, func.name
            )
        })?;
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
                    let shape = shape.clone();
                    Box::pin(async move {
                        route_workload_call(
                            accessor, &state, interface, func, &shape, params, results,
                        )
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
/// No failure here traps the plugin. Every exit either returns the callee's own
/// answer or writes a failure into the call's result slot ([`report_failure`]),
/// because none of what can go wrong — another workload's guest trapping,
/// stopping mid-call, or never exporting the interface — is the plugin's fault,
/// and the store it would fault is shared by every workload the plugin serves.
async fn route_workload_call(
    accessor: &Accessor<SharedCtx>,
    state: &ComponentHostPluginState,
    interface: Arc<str>,
    func: Arc<str>,
    shape: &ErrorShape,
    params: &[Val],
    results: &mut [Val],
) -> wasmtime::Result<()> {
    let task = accessor.with(|mut access| {
        let mut store = access.as_context_mut();
        caller_root_task(&mut store)
    });
    let job = task.and_then(|task| state.registry()?.job_for_task(task));

    let held = match state.workload_calls.current_target(task, job, &interface) {
        Some(held) => Ok(held),
        // No handle for this interface: the call belongs to whichever workload's
        // capability call the task is serving. That workload is by definition
        // running (it is mid-call), but it need not export what the plugin is
        // calling — the one path on which that is still an open question.
        None => match task.and_then(|task| state.registry()?.caller_for_task(task)) {
            None => Err((
                CallFailure::NoTarget,
                format!(
                    "host component plugin '{}' called {interface}#{func} with no workload to \
                     call: it is not serving a workload's capability call, and holds no \
                     wasmcloud:host/workload-call target handle for {interface}",
                    state.id
                ),
            )),
            Some(caller) => match state.workload_calls.routes_for(&caller.workload_id) {
                // A lifecycle hook carries no component id: the host is telling
                // the plugin *about* a workload rather than running one of that
                // workload's calls. Worth saying so, because the hook is the one
                // context where the caller is a workload that is deliberately
                // not callable, and the generic message would read as a
                // misconfiguration.
                None if caller.component_id.is_none() => Err((
                    CallFailure::NotRunning,
                    format!(
                        "host component plugin '{}' cannot call {interface}#{func} on workload \
                         '{}' from a workload-lifecycle hook: a workload is callable only while \
                         it is running, and a hook is delivered either side of that — still \
                         deploying at on-workload-bind, already torn down at on-workload-unbind. \
                         Reclaim per-workload state by id instead.",
                        state.id, caller.workload_id
                    ),
                )),
                None => Err((
                    CallFailure::NotRunning,
                    format!(
                        "host component plugin '{}' cannot call {interface}#{func} back on its \
                         caller '{}': it is not running",
                        state.id, caller.workload_id
                    ),
                )),
                Some((generation, routes)) => match routes.get(&*interface) {
                    Some(route) => Ok((caller.workload_id, generation, Arc::clone(route))),
                    None => Err((
                        CallFailure::NotExported,
                        format!(
                            "host component plugin '{}' cannot call {interface}#{func} back on its \
                             caller '{}': that workload does not export {interface}. Open a \
                             wasmcloud:host/workload-call target for a workload that does — `callable` \
                             reports which serve it.",
                            state.id, caller.workload_id
                        ),
                    )),
                },
            },
        },
    };

    let (target, generation, route) = match held {
        Ok(held) => held,
        Err((failure, detail)) => return report_failure(results, shape, failure, detail),
    };

    // Liveness is a separate question from routing, and only routing is
    // snapshotted: a deployment that stopped since the handle opened must not be
    // instantiated again just because the handle still remembers how.
    if !state.workload_calls.is_current(&target, generation) {
        return report_failure(
            results,
            shape,
            CallFailure::NotRunning,
            format!(
                "host component plugin '{}' cannot call {interface}#{func} on workload '{target}': \
                 the deployment this call names is no longer running",
                state.id
            ),
        );
    }

    // The interface is settled — a handle only exists for one the workload
    // serves, and the fallback above resolved it — so only the function can
    // still be missing, and a workload exporting the interface without every
    // function the plugin imports never deploys.
    let Some(invocation) = route.funcs.get(&*func).map(Arc::clone) else {
        return report_failure(
            results,
            shape,
            CallFailure::Other,
            format!(
                "host component plugin '{}' cannot call {interface}#{func} on workload '{target}': \
                 the interface is served but the function is not",
                state.id
            ),
        );
    };

    if let Err(e) = invoke_linked_async_export(accessor, params, results, &invocation).await {
        // The callee's own failure — a guest trap, a store that could not be
        // built, a call past the host's ceiling. Reported rather than
        // propagated: this is the case the whole no-trap contract exists for.
        warn!(
            id = state.id,
            %target,
            %interface,
            %func,
            err = ?e,
            "call into a workload failed; reporting it to the plugin"
        );
        return report_failure(
            results,
            shape,
            CallFailure::Failed,
            format!("call to {interface}#{func} on workload '{target}' failed: {e:#}"),
        );
    }
    Ok(())
}

/// Install the `wasmcloud:host/workload-call` import on the plugin's own
/// linker: the `target` handle that names which workload this task's calls go
/// to, and `callable` for a plugin's own background work to discover what it
/// may dispatch to. A plugin that does not import the interface leaves these
/// definitions unused.
pub(super) fn install_host_workload(
    linker: &mut Linker<SharedCtx>,
    state: &Arc<ComponentHostPluginState>,
) -> anyhow::Result<()> {
    let mut instance = linker
        .instance(HOST_WORKLOAD_CALL_INTERFACE)
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
        .map_err(|e| e.context("failed to register wasmcloud:host/workload-call target"))?;

    let open_state = Arc::clone(state);
    instance
        .func_new(
            "[static]target.open",
            move |mut store, _ty, params, results| {
                let (workload_id, interface) = match params {
                    [Val::String(id), Val::String(interface)] => (
                        Arc::<str>::from(id.as_str()),
                        Arc::<str>::from(interface.as_str()),
                    ),
                    _ => wasmtime::bail!("target.open expects a workload id and an interface"),
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
                // Resolving the pair here rather than on the call is the whole
                // point: a handle exists only if this workload serves this
                // interface, so holding one is proof the call can be routed and
                // `none` is where a plugin finds out otherwise. It also pins
                // *which* component serves it, so that cannot move under a call
                // the handle scopes.
                let Some((generation, route)) = open_state
                    .workload_calls
                    .route_for(&workload_id, &interface)
                else {
                    if let Some(slot) = results.first_mut() {
                        *slot = Val::Option(None);
                    }
                    return Ok(());
                };
                let handle = store.data_mut().table.push(TargetHandle {
                    workload_id: Arc::clone(&workload_id),
                    interface: Arc::clone(&interface),
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
                        interface,
                        generation,
                        route,
                        job,
                    },
                );
                if let Some(slot) = results.first_mut() {
                    *slot = Val::Option(Some(Box::new(Val::Resource(any))));
                }
                Ok(())
            },
        )
        .map_err(|e| {
            e.context("failed to define wasmcloud:host/workload-call#[static]target.open")
        })?;

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
            e.context(
                "failed to define wasmcloud:host/workload-call#[method]target.get-workload-id",
            )
        })?;

    instance
        .func_new(
            "[method]target.get-interface",
            move |mut store, _ty, params, results| {
                let Some(Val::Resource(any)) = params.first() else {
                    wasmtime::bail!("target.get-interface expects a target handle");
                };
                let handle = any.try_into_resource::<TargetHandle>(store.as_context_mut())?;
                let interface = store.data().table.get(&handle)?.interface.to_string();
                if let Some(slot) = results.first_mut() {
                    *slot = Val::String(interface);
                }
                Ok(())
            },
        )
        .map_err(|e| {
            e.context("failed to define wasmcloud:host/workload-call#[method]target.get-interface")
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
        .map_err(|e| e.context("failed to define wasmcloud:host/workload-call#callable"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Handles nest: the innermost names the target, dropping it restores the
    /// one it shadowed, and dropping the last leaves no target at all. Dropped
    /// out of order, each still removes its own entry rather than whichever is
    /// on top — a guest is free to drop them in any order.
    /// A frame for `IFACE`, which is what every handle in these tests routes.
    fn test_frame(rep: u32, id: &str, job: Option<u64>) -> TargetFrame {
        TargetFrame {
            rep,
            workload_id: Arc::from(id),
            interface: Arc::from(IFACE),
            generation: 0,
            route: Arc::new(InterfaceRoute {
                component_name: Arc::from("test-component"),
                funcs: BTreeMap::new(),
            }),
            job,
        }
    }

    const IFACE: &str = "acme:events/handler@0.1.0";

    #[test]
    fn target_handles_nest_and_unwind() {
        let calls = WorkloadCalls::new("test-plugin", Vec::new());
        let frame = test_frame;
        let held = |calls: &WorkloadCalls, job: Option<u64>| {
            calls
                .current_target(None, job, IFACE)
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
        calls.push_target(None, test_frame(1, "wl-other", Some(1)));

        // Job 2 is assigned the same task id and holds no handle of its own.
        assert!(
            calls.current_target(None, Some(2), IFACE).is_none(),
            "a later call must not inherit a leaked frame from an earlier job"
        );
        // The frame is still the target of its own job, which may still be
        // running — only the task id was reused.
        assert_eq!(
            calls
                .current_target(None, Some(1), IFACE)
                .map(|(workload_id, ..)| workload_id.to_string())
                .as_deref(),
            Some("wl-other")
        );
    }

    /// A handle routes the interface it was opened for and nothing else. A call
    /// on another interface falls back to the caller, exactly as if no handle
    /// were held — which is what keeps a handle from silently redirecting a
    /// call the workload it names cannot serve.
    #[test]
    fn a_handle_routes_only_the_interface_it_was_opened_for() {
        let calls = WorkloadCalls::new("test-plugin", Vec::new());
        calls.push_target(None, test_frame(1, "wl-a", Some(1)));

        assert!(
            calls.current_target(None, Some(1), IFACE).is_some(),
            "the interface the handle was opened for routes through it"
        );
        assert!(
            calls
                .current_target(None, Some(1), "acme:events/metrics@0.1.0")
                .is_none(),
            "another interface does not, so it falls back to the caller"
        );
    }

    /// `open` resolves a workload *and* an interface together, so a handle only
    /// exists for a pair the host has agreed to. Asking for an interface the
    /// workload does not serve hands back nothing, rather than a handle that
    /// fails later on the call.
    #[test]
    fn a_target_resolves_the_workload_and_interface_together() {
        let calls = WorkloadCalls::new("test-plugin", Vec::new());
        calls.insert_route(
            "wl-a",
            Arc::from(IFACE),
            InterfaceRoute {
                component_name: Arc::from("events-caller"),
                funcs: BTreeMap::new(),
            },
        );

        assert!(
            calls.route_for("wl-a", IFACE).is_some(),
            "the pair the workload serves resolves"
        );
        assert!(
            calls
                .route_for("wl-a", "acme:events/metrics@0.1.0")
                .is_none(),
            "an interface it does not export has no route to open a handle on"
        );
        assert!(
            calls.route_for("wl-gone", IFACE).is_none(),
            "nor has a workload that is not callable at all"
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

    /// An `async func` returning a result the host can fail into installs —
    /// what the `events-plugin` fixture and every real workload-facing import
    /// look like. The shim captures that shape, so it needs a genuine result
    /// type rather than a hand-built one.
    #[test]
    fn an_async_workload_facing_import_installs() {
        use super::super::tests::{idle_state, introspected_interface};
        use crate::engine::Engine;

        let engine = Engine::builder().build().expect("failed to build engine");
        let mut linker = Linker::new(engine.inner());
        let state = idle_state("async-workload-call-plugin");
        let iface = introspected_interface(
            r#"
            (component
                (import "acme:events/handler@0.1.0" (instance
                    (export "notify" (func async (param "m" string)
                        (result (result string (error string)))))
                ))
            )
        "#,
        );

        add_workload_calls_to_linker(&mut linker, &state, &iface)
            .expect("an async workload-facing import should install");
    }

    /// The two error shapes the host accepts, and the ones it does not. A
    /// function that cannot carry a failure has no place to put the answer when
    /// the callee traps, which is the whole reason load refuses it.
    #[test]
    fn error_shape_accepts_a_string_or_the_call_error_variant() {
        use super::super::tests::introspected_interface;

        let shape_of = |result: &str| {
            let iface = introspected_interface(&format!(
                r#"
                (component
                    (import "acme:events/handler@0.1.0" (instance
                        (export "notify" (func async (param "m" string) {result}))
                    ))
                )
            "#
            ));
            error_shape(&iface.funcs[0].result_tys)
        };

        assert!(
            matches!(
                shape_of("(result (result string (error string)))"),
                Some(ErrorShape::Message)
            ),
            "result<_, string> is the flattened form"
        );
        assert!(
            shape_of("(result string)").is_none(),
            "a bare value has nowhere to report a failure"
        );
        assert!(
            shape_of("").is_none(),
            "nor has a function returning nothing"
        );
        assert!(
            shape_of("(result (result string (error u32)))").is_none(),
            "the host cannot synthesise an error type it does not know how to build"
        );

        // All four `result` forms the component model spells, each carrying an
        // error arm the host can build — the payload-less two through `Empty`.
        assert!(
            matches!(
                shape_of("(result (result string (error string)))"),
                Some(ErrorShape::Message)
            ),
            "result<T, E>"
        );
        assert!(
            matches!(
                shape_of("(result (result (error string)))"),
                Some(ErrorShape::Message)
            ),
            "result<_, E>"
        );
        assert!(
            matches!(
                shape_of("(result (result string))"),
                Some(ErrorShape::Empty)
            ),
            "result<T> — the error arm is there, it just carries nothing"
        );
        assert!(
            matches!(shape_of("(result (result))"), Some(ErrorShape::Empty)),
            "a bare result — what `wasi:cli/run` returns"
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
            calls.routes_for("wl-a").is_none(),
            "an unknown workload is not callable, so `open` hands out no handle"
        );
    }
}
