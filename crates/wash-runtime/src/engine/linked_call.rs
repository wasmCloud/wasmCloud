//! Cross-component dynamic-linker call machinery.
//!
//! When one component in a workload imports a function that another component
//! exports, the linker wires the import to one of the `invoke_*` helpers here.
//! Each call is dispatched, by signature, down one of two paths:
//!
//! - the **shared-store path** ([`invoke_shared_store_linked_export`] /
//!   [`invoke_linked_sync_export`]), where the callee was pre-instantiated into
//!   the caller's long-lived store and handles can cross the boundary by
//!   identity, and
//! - the **ephemeral path** ([`invoke_ephemeral_linked_export`]), where a
//!   plain-value call runs in a throwaway store built per call.
//!
//! Store creation for both paths is also here: [`ComponentCtxTemplate`] is the
//! cheap recipe for a component's [`Ctx`], [`build_ctx_from_template`] turns one
//! into a [`Ctx`], and [`new_store_from_templates`] / [`new_ephemeral_store`]
//! assemble the store (pre-instantiating the linked components). See
//! [`EphemeralLinkedCall`] for how the ephemeral path is captured at link time.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::{debug, trace};
use wasmtime::component::{
    Accessor, ComponentExportIndex, InstancePre, Val,
    types::{ComponentFunc, Type},
};
use wasmtime::error::Context as _;
use wasmtime::{AsContext, AsContextMut, StoreContextMut};
use wasmtime_wasi::WasiCtxBuilder;

use crate::engine::abandon::{AbandonedCallPolicy, arm_epoch_deadline};
#[cfg(feature = "wasi-tls")]
use crate::engine::ctx::SharedTlsProvider;
use crate::engine::ctx::{AccessorActiveCtxGuard, Ctx, SharedCtx, StoreActiveCtxGuard};
use crate::engine::instance_driver::{InstanceJob, LinkedJob};
use crate::engine::instance_pool::{self, ComponentInstance, Declined, Dispatch, InstancePool};
use crate::engine::store::relocate::{self, Relocated, bridgeable_element_type};
use crate::engine::store::stream_pump::Done;
use crate::engine::value::{carries_cross_store_handle, lift_results, lower_params};
use crate::engine::volumes::{ResolvedVolumeMount, resolve_component_volume_mounts_in_map};
use crate::engine::workload::{WorkloadComponent, WorkloadMetadata};
use crate::plugin::HostPlugin;
use crate::sockets::{self, loopback};

/// A cheap, cloneable recipe for building a component's [`Ctx`].
///
/// Constructing a [`Ctx`] is comparatively expensive (it canonicalizes volume
/// mounts, builds a fresh `WasiCtx`, sockets ctx, etc.), and a single store may
/// need a ctx for the active component *and* for each component linked into it.
/// Rather than re-derive those inputs from [`WorkloadMetadata`] every time, we
/// snapshot the per-component pieces once into this template via
/// [`ComponentCtxTemplate::from_metadata`] and hand it to
/// [`build_ctx_from_template`], which turns it into an actual [`Ctx`] for a
/// given `store_id`.
///
/// Templates drive store creation on both linked-call paths:
/// [`new_store_from_templates`] builds the long-lived request/service store
/// (one active template + the linked templates), and the ephemeral path
/// rebuilds templates per call from metadata inside [`new_ephemeral_store`].
/// The `tls_provider` field is populated (under `wasi-tls`) at the
/// [`EphemeralLinkedCall`] construction site so the ephemeral path doesn't drop
/// TLS support that the request path has.
#[derive(Clone)]
pub(crate) struct ComponentCtxTemplate {
    component_id: Arc<str>,
    workload_id: Arc<str>,
    local_resources: crate::types::LocalResources,
    volume_mounts: Vec<ResolvedVolumeMount>,
    plugins: Option<HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>>,
    loopback: Arc<std::sync::Mutex<loopback::Network>>,
    /// The host-level half of this component's socket policy: enforcement mode,
    /// address ranges, whether host-loopback access is enabled at all, and the
    /// budget. The workload-level half (`allowedHosts`,
    /// `allowedHostLoopbackPorts`) comes from `local_resources` and is layered over
    /// this when the check is built.
    socket_policy: Arc<crate::sockets::policy::SocketPolicy>,
    /// The host-wide guest memory budget this component's stores draw on.
    guest_memory: Arc<crate::engine::guest_memory::GuestMemoryBudget>,
    #[cfg(feature = "wasi-tls")]
    tls_provider: Option<SharedTlsProvider>,
}

impl ComponentCtxTemplate {
    fn from_metadata(metadata: &WorkloadMetadata) -> Self {
        Self {
            component_id: metadata.id.clone(),
            workload_id: metadata.workload_id.clone(),
            local_resources: metadata.local_resources.clone(),
            volume_mounts: metadata.resolved_volume_mounts.clone(),
            plugins: metadata.plugins.clone(),
            loopback: metadata.loopback.clone(),
            socket_policy: metadata.socket_policy.clone(),
            guest_memory: metadata.guest_memory.clone(),
            #[cfg(feature = "wasi-tls")]
            tls_provider: None,
        }
    }
}

#[cfg(not(feature = "wasi-tls"))]
pub(crate) fn component_ctx_template_from_metadata(
    metadata: &WorkloadMetadata,
) -> ComponentCtxTemplate {
    ComponentCtxTemplate::from_metadata(metadata)
}

#[cfg(feature = "wasi-tls")]
pub(crate) fn component_ctx_template_from_metadata_with_tls(
    metadata: &WorkloadMetadata,
    tls_provider: Option<SharedTlsProvider>,
) -> ComponentCtxTemplate {
    let mut template = ComponentCtxTemplate::from_metadata(metadata);
    template.tls_provider = tls_provider;
    template
}

/// Everything needed to spin up a throwaway store for a single cross-component
/// linked call.
///
/// # Where it fits in a cross-component call
///
/// When a component (`active_component_id`) imports a function that another
/// component in the same workload exports, the dynamic linker routes the call
/// to one of two paths, chosen at link time by [`func_is_ephemeral_safe`]:
///
/// - **Shared-store path** — used when the call's signature carries a handle
///   that must keep its identity across the boundary (resource/borrow/stream/
///   future/error-context; see [`carries_cross_store_handle`]). The callee is
///   instantiated once into the caller's long-lived store and reused
///   ([`invoke_shared_store_linked_export`]).
/// - **Ephemeral path** — used when every parameter and result is a *plain
///   value* (no cross-store handle). The call runs in a brand-new store that is
///   instantiated, invoked, and dropped per call
///   ([`invoke_ephemeral_linked_export`]), so its core-instance slots are
///   reclaimed immediately. Plain values copy cleanly across the store
///   boundary, so nothing is lost by not sharing a store.
///
/// This struct is the captured input for that second path. One
/// `Arc<EphemeralLinkedCall>` is built per eligible import during
/// `link_components` and stored on the [`LinkedExportInvocation`]; each call
/// hands it to [`new_ephemeral_store`], which rebuilds the active + linked
/// [`ComponentCtxTemplate`]s from current metadata (`components`),
/// pre-instantiates the linked components into the fresh store, and runs the
/// export. Wrapped in `Arc` so the per-call clone is a pointer bump rather than
/// a deep copy of the engine/handler/component map.
#[derive(Clone)]
pub(crate) struct EphemeralLinkedCall {
    pub(crate) engine: wasmtime::Engine,
    pub(crate) http_handler: Arc<dyn crate::host::http::HostHandler>,
    pub(crate) components: Arc<RwLock<BTreeMap<Arc<str>, WorkloadComponent>>>,
    pub(crate) active_component_id: Arc<str>,
    pub(crate) linked_component_ids: Vec<Arc<str>>,
    #[cfg(feature = "wasi-tls")]
    pub(crate) tls_provider: Option<SharedTlsProvider>,
    /// How this call moves its args/results across the store boundary.
    pub(crate) mode: EphemeralCallMode,
}

/// How an ephemeral linked call transfers its args/results across the store
/// boundary, decided by the signature classification at link time.
#[derive(Clone)]
pub(crate) enum EphemeralCallMode {
    /// Handle-free call: params/results are copied directly.
    PlainValue,
    /// The signature carries a bridgeable `stream<T>` or `future<T>`, so
    /// args/results are relocated across the boundary (see [`relocate`]), driven
    /// by these param/result types.
    Relocated {
        param_tys: Arc<[Type]>,
        result_tys: Arc<[Type]>,
    },
}

fn type_is_ephemeral_safe(ty: &Type) -> bool {
    !carries_cross_store_handle(ty)
}

/// Whether every one of `tys` is a plain value, so a call carrying them copies
/// cleanly into an ephemeral store. The type-list form of
/// [`func_is_ephemeral_safe`], for a caller holding params and results
/// separately rather than as one [`ComponentFunc`].
#[cfg(feature = "host-component-plugins")]
pub(crate) fn types_are_ephemeral_safe(tys: &[Type]) -> bool {
    tys.iter().all(type_is_ephemeral_safe)
}

pub(crate) fn func_is_ephemeral_safe(func_ty: &ComponentFunc) -> bool {
    func_ty.params().all(|(_, ty)| type_is_ephemeral_safe(&ty))
        && func_ty.results().all(|ty| type_is_ephemeral_safe(&ty))
}

/// Whether a type can cross an ephemeral-store boundary via [`relocate`].
///
/// True when the type is either:
/// - handle-free, or
/// - carrying only `stream<T>`/`future<T>` handles whose element type is
///   relocatable (nested anywhere in aggregates).
///
/// `resource` (`own`/`borrow`) and `error-context` handles are not relocatable
/// between two ephemeral-call stores, so a type carrying either is not
/// bridge-safe. (A `resource` crosses only the host-component-plugin bridge,
/// where a plugin-side registry exists — see
/// [`crate::engine::store::resource_bridge`].)
fn type_is_bridge_safe(ty: &Type) -> bool {
    if !carries_cross_store_handle(ty) {
        return true;
    }
    match ty {
        Type::Stream(st) => st.ty().is_some_and(|e| bridgeable_element_type(&e)),
        Type::Future(ft) => ft.ty().is_some_and(|e| bridgeable_element_type(&e)),
        Type::List(t) => type_is_bridge_safe(&t.ty()),
        Type::Option(t) => type_is_bridge_safe(&t.ty()),
        Type::Tuple(t) => t.types().all(|t| type_is_bridge_safe(&t)),
        Type::Record(t) => t.fields().all(|f| type_is_bridge_safe(&f.ty)),
        Type::Variant(t) => t
            .cases()
            .all(|c| c.ty.is_none_or(|t| type_is_bridge_safe(&t))),
        Type::Result(t) => {
            t.ok().is_none_or(|t| type_is_bridge_safe(&t))
                && t.err().is_none_or(|t| type_is_bridge_safe(&t))
        }
        Type::Map(t) => type_is_bridge_safe(&t.key()) && type_is_bridge_safe(&t.value()),
        // resource (own/borrow) / error-context: not relocatable here.
        _ => false,
    }
}

/// Whether every one of `tys` is [`type_is_bridge_safe`]. The type-list form of
/// [`func_is_bridge_safe`], for a caller holding params and results separately
/// rather than as one [`ComponentFunc`].
#[cfg(feature = "host-component-plugins")]
pub(crate) fn types_are_bridge_safe(tys: &[Type]) -> bool {
    tys.iter().all(type_is_bridge_safe)
}

/// Whether every param/result of `func_ty` is [`type_is_bridge_safe`], so a call
/// carrying a `stream<T>`/`future<T>` can still run in an ephemeral store (with
/// relocation) instead of being pinned to the shared store.
pub(crate) fn func_is_bridge_safe(func_ty: &ComponentFunc) -> bool {
    func_ty.params().all(|(_, ty)| type_is_bridge_safe(&ty))
        && func_ty.results().all(|ty| type_is_bridge_safe(&ty))
}

async fn build_ctx_from_template(
    template: &ComponentCtxTemplate,
    http_handler: Arc<dyn crate::host::http::HostHandler>,
    all_volume_mounts: &[ResolvedVolumeMount],
    store_id: &str,
    is_service: bool,
) -> anyhow::Result<Ctx> {
    let mut wasi_ctx_builder = WasiCtxBuilder::new();
    wasi_ctx_builder
        .envs(
            template
                .local_resources
                .environment
                .iter()
                .map(|kv| (kv.0.as_str(), kv.1.as_str()))
                .collect::<Vec<_>>()
                .as_slice(),
        )
        .inherit_stdout()
        .inherit_stderr();

    let kind = if is_service {
        sockets::policy::GuestKind::Service
    } else {
        sockets::policy::GuestKind::Component
    };
    // Keyed on the workload, not the component: a workload's components share
    // one allowance, the same way they share one virtual network.
    let policy = Arc::new(sockets::policy::SocketPolicy {
        allowed_hosts: Arc::clone(&template.local_resources.allowed_hosts),
        host_loopback: Arc::clone(&template.local_resources.allowed_host_loopback_ports),
        ..template
            .socket_policy
            .for_guest(kind, &template.workload_id)
    });
    let sockets_ctx = sockets::WasiSocketsCtx {
        socket_addr_check: sockets::SocketAddrCheck::new(move |addr, reason| {
            let policy = Arc::clone(&policy);
            Box::pin(async move { policy.decide(reason, addr) })
        }),
        loopback: Arc::clone(&template.loopback),
        allowed_ip_name_lookups: Arc::clone(&template.local_resources.allowed_ip_name_lookups),
        ..Default::default()
    };

    for mount in all_volume_mounts {
        wasi_ctx_builder.preopened_dir(
            &mount.host_path,
            &mount.mount_path,
            mount.dir_perms,
            mount.file_perms,
        )?;
    }

    let mut ctx_builder = Ctx::builder(template.workload_id.clone(), template.component_id.clone())
        .with_http_handler(http_handler)
        .with_wasi_ctx(wasi_ctx_builder.build())
        .with_sockets(sockets_ctx)
        .with_allowed_hosts(template.local_resources.allowed_hosts.clone());

    if let Some(plugins) = &template.plugins {
        ctx_builder = ctx_builder.with_plugins(plugins.clone());
    }

    #[cfg(feature = "wasi-tls")]
    if let Some(provider) = template.tls_provider.clone() {
        ctx_builder = ctx_builder.with_tls_provider(provider);
    }

    let mut ctx = ctx_builder.build();
    ctx.store_id = store_id.to_string().into();
    Ok(ctx)
}

pub(crate) async fn new_store_from_templates(
    engine: &wasmtime::Engine,
    http_handler: Arc<dyn crate::host::http::HostHandler>,
    active: &ComponentCtxTemplate,
    linked: &[ComponentCtxTemplate],
    linked_instances: &[(Arc<str>, InstancePre<SharedCtx>)],
    is_service: bool,
) -> anyhow::Result<wasmtime::Store<SharedCtx>> {
    let store_id = uuid::Uuid::new_v4().to_string();
    let all_volume_mounts = std::iter::once(active)
        .chain(linked.iter())
        .flat_map(|template| template.volume_mounts.clone())
        .collect::<Vec<_>>();
    let active_ctx = build_ctx_from_template(
        active,
        http_handler.clone(),
        &all_volume_mounts,
        &store_id,
        is_service,
    )
    .await?;
    let mut shared_ctx = SharedCtx::new(active_ctx).with_guest_memory(&active.guest_memory);

    for linked in linked {
        let linked_ctx = build_ctx_from_template(
            linked,
            http_handler.clone(),
            &all_volume_mounts,
            &store_id,
            false,
        )
        .await?;
        shared_ctx
            .contexts
            .insert(linked.component_id.clone(), linked_ctx);
    }

    let mut store = wasmtime::Store::new(engine, shared_ctx);
    // A store on a fuel-metering engine starts at zero, and calling a guest
    // without fuel traps. Fuel here is a counter, never a bound:
    // `FuelConsumptionMeter::observe` resets it and reads the delta around the
    // call it measures, and nothing else reads it at all. Giving every store
    // the maximum is what lets a guest run while its consumption is counted.
    // Errors when the engine is not metering fuel, which is the ordinary case.
    let _ = store.set_fuel(u64::MAX);
    // Trap for every store built here, services included: trapping a service
    // means a supervisor restart, which beats carrying a wedged call forever.
    arm_epoch_deadline(&mut store, AbandonedCallPolicy::Trap);
    crate::engine::guest_memory::install_memory_limiter(&mut store);

    let active_id = active.component_id.clone();
    for (linked_id, linked_pre) in linked_instances {
        store.data_mut().set_active_ctx(linked_id)?;
        let instantiate_result = linked_pre.instantiate_async(&mut store).await;
        store.data_mut().set_active_ctx(&active_id)?;
        let instance = instantiate_result.map_err(|e| {
            anyhow::anyhow!(
                "failed to instantiate linked component '{linked_id}' in ephemeral store: {e}"
            )
        })?;
        store
            .data_mut()
            .exporter_instances
            .insert(linked_id.clone(), instance);
    }

    Ok(store)
}

/// The callee's warm-instance pool, or `None` when this store must not be
/// parked — see [`instance_pool::poolable`], which also accounts for the linked
/// components instantiated into the same store.
///
/// Read from the component map per call rather than captured at link time, so
/// a component whose entry is replaced does not keep serving from a pool that
/// belongs to the old entry.
async fn callee_instance_pool(call: &EphemeralLinkedCall) -> Option<Arc<InstancePool>> {
    let components = call.components.read().await;
    let linked: HashSet<Arc<str>> = call.linked_component_ids.iter().cloned().collect();
    instance_pool::poolable(&components, &call.active_component_id, &linked)
}

/// What a linked call is measured under, or an empty set when nothing will
/// record it.
///
/// The identity is the *callee's*, because it is the callee's guest code the
/// histogram times, and every path that runs that code uses this — pooled or in
/// a store of its own, plain args or relocated ones.
///
/// Nothing records unless the host chose the epoch meter, so on every other
/// host this is a read lock, a `format!` and six allocations per linked call
/// for a value no one reads. An empty set is what a sample that records nothing
/// needs.
async fn linked_attributes(
    call: &EphemeralLinkedCall,
    inv: &LinkedExportInvocation,
) -> Arc<[opentelemetry::KeyValue]> {
    let Some(identity) = callee_identity(call).await else {
        return Arc::from([]);
    };
    identity.attributes(
        "linked",
        &format!("{}#{}", inv.import_name, inv.export_name),
    )
}

async fn new_ephemeral_store(
    call: &EphemeralLinkedCall,
) -> anyhow::Result<wasmtime::Store<SharedCtx>> {
    let mut component_ids = call.linked_component_ids.clone();
    component_ids.push(call.active_component_id.clone());
    component_ids.sort();
    component_ids.dedup();
    resolve_component_volume_mounts_in_map(&call.components, &component_ids).await?;

    // One read-lock scope, no `WorkloadMetadata` clones: cloning metadata would
    // deep-clone its by-value `Linker`, and `pre_instantiate_ref` needs only
    // read access, so concurrent ephemeral calls don't serialize on a write
    // lock. Nothing is retained past this store.
    #[cfg(feature = "wasi-tls")]
    let template_of = |metadata: &WorkloadMetadata| {
        component_ctx_template_from_metadata_with_tls(metadata, call.tls_provider.clone())
    };
    #[cfg(not(feature = "wasi-tls"))]
    let template_of = component_ctx_template_from_metadata;

    let (active, linked, linked_instances) = {
        let components = call.components.read().await;
        let active = template_of(
            &components
                .get(&call.active_component_id)
                .with_context(|| {
                    format!(
                        "ephemeral linked component '{}' not found",
                        call.active_component_id
                    )
                })?
                .metadata,
        );
        let mut linked = Vec::with_capacity(call.linked_component_ids.len());
        let mut linked_instances = Vec::with_capacity(call.linked_component_ids.len());
        for component_id in &call.linked_component_ids {
            let component = components
                .get(component_id)
                .with_context(|| format!("linked component '{component_id}' not found"))?;
            linked.push(template_of(&component.metadata));
            linked_instances.push((
                component_id.clone(),
                component.pre_instantiate_ref().map_err(|e| {
                    anyhow::anyhow!(
                        "failed to pre-instantiate linked components for ephemeral call: {e}"
                    )
                })?,
            ));
        }
        (active, linked, linked_instances)
    };

    let store = new_store_from_templates(
        &call.engine,
        call.http_handler.clone(),
        &active,
        &linked,
        &linked_instances,
        false,
    )
    .await?;
    if let Some(identity) = callee_identity(call).await {
        store.data().executed.set_identity(identity);
    }
    Ok(store)
}

/// The callee's manifest identity, for stamping a store or naming a call.
///
/// `None` when nothing will read it, or when the component has left the map —
/// a call racing a teardown, which is about to fail anyway.
async fn callee_identity(
    call: &EphemeralLinkedCall,
) -> Option<crate::observability::WorkloadIdentity> {
    crate::observability::invocation_meter()?;
    let components = call.components.read().await;
    let component = components.get(&call.active_component_id)?;
    Some(crate::observability::WorkloadIdentity::new(
        component.metadata().workload_namespace(),
        component.metadata().workload_name(),
        component.name(),
    ))
}

#[derive(Clone)]
pub(crate) struct LinkedExportInvocation {
    pub(crate) import_name: Arc<str>,
    pub(crate) export_name: Arc<str>,
    pub(crate) pre: InstancePre<SharedCtx>,
    pub(crate) plugin_component_id: Arc<str>,
    pub(crate) func_idx: ComponentExportIndex,
    pub(crate) param_tys: Arc<std::sync::OnceLock<Arc<[Type]>>>,
    pub(crate) ephemeral_call: Option<Arc<EphemeralLinkedCall>>,
}

pub(crate) async fn invoke_linked_async_export(
    accessor: &Accessor<SharedCtx>,
    params: &[Val],
    results: &mut [Val],
    inv: &LinkedExportInvocation,
) -> wasmtime::Result<()> {
    if let Some(ephemeral_call) = &inv.ephemeral_call {
        invoke_ephemeral_linked_export(accessor, params, results, inv, ephemeral_call).await
    } else {
        invoke_shared_store_linked_export(accessor, params, results, inv).await
    }
}

/// Aborts the wrapped task when dropped before it completes, so a cancelled
/// caller (e.g. a client disconnect tearing down the request future) reclaims
/// the ephemeral store's core-instance slots immediately instead of leaving a
/// detached task to run to its timeout.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Dispatch an ephemeral linked call to either the plain-value copy path or the
/// `stream`-relocating path, by the signature classification recorded at link
/// time.
async fn invoke_ephemeral_linked_export(
    accessor: &Accessor<SharedCtx>,
    params: &[Val],
    results: &mut [Val],
    inv: &LinkedExportInvocation,
    ephemeral_call: &Arc<EphemeralLinkedCall>,
) -> wasmtime::Result<()> {
    match &ephemeral_call.mode {
        EphemeralCallMode::Relocated {
            param_tys,
            result_tys,
        } => {
            invoke_ephemeral_relocated(
                accessor,
                params,
                results,
                inv,
                ephemeral_call,
                Arc::clone(param_tys),
                Arc::clone(result_tys),
            )
            .await
        }
        EphemeralCallMode::PlainValue => {
            invoke_ephemeral_plain(params, results, inv, ephemeral_call).await
        }
    }
}

/// Run a `stream`-carrying async linked call in an ephemeral store, relocating
/// args/results across the boundary (see [`relocate`]).
///
/// Args are extracted in the caller store, so each source stream begins pumping
/// under the caller's long-lived runtime; the call then runs in a throwaway
/// store, where result streams are extracted before the store is torn down. The
/// store-driving task is **detached** (leaked after initial [`AbortOnDrop`]
/// wrapping): it must outlive
/// this call to keep producing into result streams while the caller consumes
/// them. It self-terminates when a result stream's consumer is dropped — which
/// closes the pump channel — so caller cancellation still reclaims the store.
async fn invoke_ephemeral_relocated(
    accessor: &Accessor<SharedCtx>,
    params: &[Val],
    results: &mut [Val],
    inv: &LinkedExportInvocation,
    ephemeral_call: &Arc<EphemeralLinkedCall>,
    param_tys: Arc<[Type]>,
    result_tys: Arc<[Type]>,
) -> wasmtime::Result<()> {
    // This path runs guest code like the plain one, so it is measured like it:
    // a signature carrying a `stream` or `future` is not a reason for a call to
    // be missing from the histogram.
    let attributes = linked_attributes(ephemeral_call, inv).await;
    // Extract args in the caller store: source-stream pumps run under the
    // caller's (long-lived) runtime, so their drain signals are dropped here.
    let args = accessor.with(|mut access| -> wasmtime::Result<Vec<Relocated>> {
        let mut dones: Vec<Done> = Vec::new();
        let mut out = Vec::with_capacity(params.len());
        for (v, t) in params.iter().zip(param_tys.iter()) {
            out.push(relocate::extract(
                access.as_context_mut(),
                v,
                t,
                &mut dones,
            )?);
        }
        Ok(out)
    })?;

    let (ready_tx, ready_rx) =
        futures::channel::oneshot::channel::<wasmtime::Result<Vec<Relocated>>>();
    let ephemeral_call = Arc::clone(ephemeral_call);
    let callee_pre = inv.pre.clone();
    let func_idx = inv.func_idx;
    let import_name = inv.import_name.clone();
    let export_name = inv.export_name.clone();

    // Deadline enforced from out here (see `crate::engine::abandon`); the flag
    // travels into the task, which registers it once the store exists.
    let call = crate::engine::abandon::DispatchedCall::new(
        "linked (relocated ephemeral store)",
        crate::timeouts::ephemeral_call(),
    );
    let call_flag = call.flag();

    trace!(
        name = %inv.import_name,
        fn_name = %inv.export_name,
        "invoking relocated ephemeral dynamic export"
    );

    // Guard the store-driving task so a caller cancelled BEFORE results are ready
    // (e.g. a client disconnect) aborts the in-flight call and reclaims the
    // ephemeral store's core-instance slots, rather than leaving it to run to its
    // timeout. Once results are handed back the task must outlive this call to
    // drain result streams, so the guard is forgotten (detached) on success.
    let task = AbortOnDrop(tokio::task::spawn(async move {
        let mut store = match new_ephemeral_store(&ephemeral_call).await {
            Ok(s) => s,
            Err(e) => {
                let _ = ready_tx.send(Err(wasmtime::format_err!("{e:#}")));
                return;
            }
        };
        // Watched for the store's whole run, the post-reply stream drain
        // included.
        let drain_flag = Arc::clone(&call_flag);
        let _abandoned = store.data().abandoned.watch(call_flag);
        let instance = match callee_pre.instantiate_async(&mut store).await {
            Ok(i) => i,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        let _ = store
            .run_concurrent(async move |accessor| {
                let ready = async {
                    // get_func + arg injection inside run_concurrent: the store is
                    // in async-required mode after instantiate.
                    let (func, arg_vals) = accessor.with(|mut access| -> wasmtime::Result<_> {
                        let func = instance.get_func(&mut access, func_idx).with_context(|| {
                            format!(
                                "function not found for linked import {import_name}.{export_name}"
                            )
                        })?;
                        let mut arg_vals = Vec::with_capacity(args.len());
                        for a in args {
                            arg_vals.push(relocate::inject(access.as_context_mut(), a)?);
                        }
                        Ok((func, arg_vals))
                    })?;
                    let _sample =
                        crate::engine::instance_driver::InvocationSample::start(attributes);
                    let mut results_buf = vec![Val::Bool(false); result_tys.len()];
                    let call_timeout = crate::timeouts::ephemeral_call();
                    timeout(
                        call_timeout,
                        func.call_concurrent(accessor, &arg_vals, &mut results_buf),
                    )
                    .await
                    .map_err(|e| {
                        wasmtime::format_err!("function call timed out after {call_timeout:?}: {e}")
                    })??;
                    // Extract result streams in THIS store before it is dropped.
                    accessor.with(
                        |mut access| -> wasmtime::Result<(Vec<Relocated>, Vec<Done>)> {
                            let mut dones: Vec<Done> = Vec::new();
                            let mut out = Vec::with_capacity(results_buf.len());
                            for (r, t) in results_buf.iter().zip(result_tys.iter()) {
                                out.push(relocate::extract(
                                    access.as_context_mut(),
                                    r,
                                    t,
                                    &mut dones,
                                )?);
                            }
                            Ok((out, dones))
                        },
                    )
                }
                .await;

                match ready {
                    Ok((relocated, dones)) => {
                        let _ = ready_tx.send(Ok(relocated));
                        // Keep the store alive until result streams drain, but bound
                        // it: a consumer that never reads (or never drops) its result
                        // stream would otherwise pin this ephemeral store — and its
                        // core-instance slots — indefinitely. A transfer still making
                        // progress past this bound is truncated when the store drops.
                        // The `timeout` below is a future on this store, so it
                        // cannot fire if the guest stops yielding mid-drain.
                        // This timer runs off-store, and is cancelled when the
                        // drain ends.
                        let _drain_timer =
                            Arc::clone(&drain_flag).arm_after(crate::timeouts::stream_drain());
                        let drain = async {
                            for done in dones {
                                let _ = done.await;
                            }
                        };
                        if timeout(crate::timeouts::stream_drain(), drain)
                            .await
                            .is_err()
                        {
                            trace!("relocated ephemeral store drain timed out; dropping store");
                        }
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
                Ok::<(), wasmtime::Error>(())
            })
            .await;
    }));

    let relocated = call
        .await_reply(ready_rx)
        .await
        .ok_or_else(|| wasmtime::format_err!("ephemeral store produced no results in time"))?
        .map_err(|_| wasmtime::format_err!("ephemeral store dropped before producing results"))??;

    // Results are in hand; the task must keep running to feed any result streams,
    // so detach it (cancellation past this point is handled by the result-stream
    // consumers closing their pump channels). The drain is bounded from inside
    // that task.
    std::mem::forget(task);

    // Inject results into the caller store; result-stream producers pull from
    // the still-draining ephemeral store.
    accessor.with(|mut access| -> wasmtime::Result<()> {
        for (i, r) in relocated.into_iter().enumerate() {
            let v = relocate::inject(access.as_context_mut(), r)?;
            *results.get_mut(i).context("result index out of bounds")? = v;
        }
        Ok(())
    })?;

    trace!(
        name = %inv.import_name,
        fn_name = %inv.export_name,
        "successfully invoked relocated ephemeral dynamic export"
    );

    Ok(())
}

/// Run a plain-value async linked call.
///
/// A component that keeps instances warm serves this on one of them, alongside
/// whatever else that instance already has in flight (see
/// [`crate::engine::instance_driver`]). Otherwise — and when every warm
/// instance is busy and the pool is full — the call runs in a store built,
/// invoked and dropped for it alone, so its core-instance slots are reclaimed
/// immediately.
async fn invoke_ephemeral_plain(
    params: &[Val],
    results: &mut [Val],
    inv: &LinkedExportInvocation,
    ephemeral_call: &EphemeralLinkedCall,
) -> wasmtime::Result<()> {
    trace!(
        name = %inv.import_name,
        fn_name = %inv.export_name,
        ?params,
        "invoking ephemeral dynamic export"
    );

    let pool = callee_instance_pool(ephemeral_call).await;
    let attributes = linked_attributes(ephemeral_call, inv).await;

    // The params travel into the pooled job. A job the pool declines hands
    // them back, so the cold path below reuses that allocation rather than
    // cloning them a second time.
    let mut declined_params = None;
    // As does an instance built for a pool that then declined the call: the
    // store of its own below is that instance.
    let mut reclaimed = None;

    if let Some(pool) = pool.as_ref() {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        // Deadline enforced here, in the caller's task, outside the callee's
        // store — where a non-yielding callee cannot block it (see
        // `crate::engine::abandon`).
        let call = crate::engine::abandon::DispatchedCall::new(
            "linked (pooled)",
            crate::timeouts::ephemeral_call(),
        );
        let job = InstanceJob::Linked(Box::new(LinkedJob {
            func_idx: inv.func_idx,
            params: params.to_vec(),
            results_len: results.len(),
            import_name: inv.import_name.clone(),
            export_name: inv.export_name.clone(),
            reply,
            abandoned: call.flag(),
            attributes: Arc::clone(&attributes),
        }));
        let outcome = match pool.try_dispatch(job) {
            Dispatch::Sent => Ok(()),
            // The pool has room. Build and instantiate the store out here,
            // where awaiting is allowed and where a component that fails to
            // instantiate reports that failure to this call rather than only
            // to the log.
            Dispatch::NeedsInstance(job) => {
                let mut store = new_ephemeral_store(ephemeral_call).await.map_err(|e| {
                    wasmtime::format_err!("new pooled store creation failed: {e:#}")
                })?;
                let instance = inv.pre.instantiate_async(&mut store).await?;
                pool.dispatch_on_new(ComponentInstance { store, instance }, job)
            }
            Dispatch::Saturated(job) => Err(Declined::without_instance(job)),
        };
        match outcome {
            Ok(()) => {
                let vals = call
                    .await_reply(reply_rx)
                    .await
                    .ok_or_else(|| {
                        wasmtime::format_err!("pooled instance produced no reply in time")
                    })?
                    .map_err(|_| wasmtime::format_err!("pooled instance dropped the call"))??;
                write_results(vals, results)?;
                trace!(
                    name = %inv.import_name,
                    fn_name = %inv.export_name,
                    ?results,
                    "invoked ephemeral dynamic export"
                );
                return Ok(());
            }
            // Every warm instance was busy; run it in a store of its own.
            Err(declined) => {
                debug!(
                    name = %inv.import_name,
                    fn_name = %inv.export_name,
                    "warm instances saturated; serving this call from a store of its own"
                );
                reclaimed = declined.instance;
                if let InstanceJob::Linked(job) = declined.job {
                    declined_params = Some(job.params);
                }
            }
        }
    }

    let ComponentInstance {
        mut store,
        instance,
    } = match reclaimed {
        Some(built) => built,
        None => {
            let mut store = new_ephemeral_store(ephemeral_call)
                .await
                .map_err(|e| wasmtime::format_err!("new ephemeral store creation failed: {e:#}"))?;
            let instance = inv.pre.instantiate_async(&mut store).await?;
            ComponentInstance { store, instance }
        }
    };

    let params_buf = declined_params.unwrap_or_else(|| params.to_vec());
    let mut results_buf = vec![Val::Bool(false); results.len()];
    let call_import_name = inv.import_name.clone();
    let call_export_name = inv.export_name.clone();
    let func_idx = inv.func_idx;

    // Deadline enforced from out here (see `crate::engine::abandon`); the
    // watch guard travels into the task to cover the call for its whole run.
    let call = crate::engine::abandon::DispatchedCall::new(
        "linked (ephemeral store)",
        crate::timeouts::ephemeral_call(),
    );
    let watch_guard = store.data().abandoned.watch(call.flag());

    // The store travels into the task, so a caller cancelled mid-call drops it
    // with the task rather than leaving it running.
    let mut task = AbortOnDrop(tokio::task::spawn(async move {
        let _abandoned = watch_guard;
        store
            .run_concurrent(async move |accessor| {
                let func = accessor.with(|mut access| -> wasmtime::Result<_> {
                    instance.get_func(&mut access, func_idx).with_context(|| {
                        format!(
                            "function not found for linked import {call_import_name}.{call_export_name}"
                        )
                    })
                })?;
                // Started once the export resolves, as on the pooled path: a
                // component that declared no pool is the default, and its calls
                // belong in the same histogram.
                let _sample = crate::engine::instance_driver::InvocationSample::start(attributes);
                let call_timeout = crate::timeouts::ephemeral_call();
                timeout(
                    call_timeout,
                    func.call_concurrent(accessor, &params_buf, &mut results_buf),
                )
                .await
                .map_err(|e| {
                    wasmtime::format_err!("function call timed out after {call_timeout:?}: {e}")
                })??;
                Ok::<Vec<Val>, wasmtime::Error>(results_buf)
            })
            .await
            .map_err(|e| wasmtime::format_err!("{e:#}"))
            .and_then(|inner| inner)
    }));
    let vals = call
        .await_reply(&mut task.0)
        .await
        .ok_or_else(|| wasmtime::format_err!("ephemeral linked call produced no result in time"))?
        .map_err(|e| wasmtime::format_err!("ephemeral linked call task failed: {e}"))??;
    write_results(vals, results)?;

    trace!(
        name = %inv.import_name,
        fn_name = %inv.export_name,
        ?results,
        "invoked ephemeral dynamic export"
    );

    Ok(())
}

/// Copy a call's returned values into the caller's result slots.
fn write_results(vals: Vec<Val>, results: &mut [Val]) -> wasmtime::Result<()> {
    for (i, v) in vals.into_iter().enumerate() {
        *results.get_mut(i).context("result index out of bounds")? = v;
    }
    Ok(())
}

async fn invoke_shared_store_linked_export(
    accessor: &Accessor<SharedCtx>,
    params: &[Val],
    results: &mut [Val],
    inv: &LinkedExportInvocation,
) -> wasmtime::Result<()> {
    let _active_ctx = AccessorActiveCtxGuard::new(accessor, &inv.plugin_component_id)?;

    let call: wasmtime::Result<()> = async {
        let (func, params_buf) = accessor.with(|mut access| -> wasmtime::Result<_> {
            let instance = access
                .data_mut()
                .exporter_instances
                .get(&inv.plugin_component_id)
                .copied()
                .with_context(|| {
                    format!(
                        "linked component '{}' was not pre-instantiated in this store",
                        inv.plugin_component_id
                    )
                })?;
            let func = instance
                .get_func(&mut access, inv.func_idx)
                .context("function not found")?;
            let tys = inv.param_tys.get_or_init(|| {
                func.ty(access.as_context())
                    .params()
                    .map(|(_, ty)| ty)
                    .collect::<Vec<_>>()
                    .into()
            });
            let params_buf = lower_params(&mut access.as_context_mut(), params, tys)?;
            Ok((func, params_buf))
        })?;

        trace!(name = %inv.import_name, fn_name = %inv.export_name, "invoking dynamic export");

        let mut results_buf = vec![Val::Bool(false); results.len()];
        func.call_concurrent(accessor, &params_buf, &mut results_buf)
            .await?;

        accessor.with(|mut access| -> wasmtime::Result<_> {
            lift_results(&mut access.as_context_mut(), results_buf, results)
        })?;

        Ok(())
    }
    .await;

    call?;

    trace!(name = %inv.import_name, fn_name = %inv.export_name, "invoked dynamic export");

    Ok(())
}

pub(crate) async fn invoke_linked_sync_export(
    store: StoreContextMut<'_, SharedCtx>,
    params: &[Val],
    results: &mut [Val],
    inv: &LinkedExportInvocation,
) -> wasmtime::Result<()> {
    let mut active_ctx = StoreActiveCtxGuard::new(store, &inv.plugin_component_id)?;
    let mut store = active_ctx.store_mut();

    async {
        let instance = store
            .data()
            .exporter_instances
            .get(&inv.plugin_component_id)
            .copied()
            .with_context(|| {
                format!(
                    "linked component '{}' was not pre-instantiated in this store",
                    inv.plugin_component_id
                )
            })?;

        let func = instance
            .get_func(&mut store, inv.func_idx)
            .context("function not found")?;
        let tys = inv.param_tys.get_or_init(|| {
            func.ty(store.as_context())
                .params()
                .map(|(_, ty)| ty)
                .collect::<Vec<_>>()
                .into()
        });
        let params_buf = lower_params(store, params, tys)?;
        trace!(name = %inv.import_name, fn_name = %inv.export_name, "invoking dynamic export");

        let mut results_buf = vec![Val::Bool(false); results.len()];

        let call_timeout = crate::timeouts::shared_store_call();
        timeout(
            call_timeout,
            func.call_async(&mut store, &params_buf, &mut results_buf),
        )
        .await
        .map_err(|e| {
            wasmtime::format_err!("function call timed out after {call_timeout:?}: {e}")
        })??;

        lift_results(store, results_buf, results)?;
        trace!(name = %inv.import_name, fn_name = %inv.export_name, "invoked dynamic export");
        Ok(())
    }
    .await
}
