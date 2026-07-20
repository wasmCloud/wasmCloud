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

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::trace;
use wasmtime::component::{
    Accessor, ComponentExportIndex, InstancePre, Val,
    types::{ComponentFunc, Type},
};
use wasmtime::error::Context as _;
use wasmtime::{AsContext, AsContextMut, StoreContextMut};
use wasmtime_wasi::WasiCtxBuilder;

#[cfg(feature = "wasi-tls")]
use crate::engine::ctx::SharedTlsProvider;
use crate::engine::ctx::{AccessorActiveCtxGuard, Ctx, SharedCtx, StoreActiveCtxGuard};
use crate::engine::store::relocate::{self, Relocated, bridgeable_element_type};
use crate::engine::store::stream_pump::Done;
use crate::engine::value::{carries_cross_store_handle, lift_results, lower_params};
use crate::engine::volumes::{ResolvedVolumeMount, resolve_component_volume_mounts_in_map};
use crate::engine::workload::{WorkloadComponent, WorkloadMetadata};
use crate::plugin::HostPlugin;
use crate::sockets::{self, SocketAddrUse, loopback};

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
    pub(crate) components: Arc<RwLock<HashMap<Arc<str>, WorkloadComponent>>>,
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

    let sockets_ctx = sockets::WasiSocketsCtx {
        socket_addr_check: sockets::SocketAddrCheck::new(move |addr, reason| {
            Box::pin(async move {
                match reason {
                    SocketAddrUse::TcpBind if is_service => addr.ip().is_loopback(),
                    SocketAddrUse::TcpBind => false,
                    SocketAddrUse::UdpBind => addr.ip().is_loopback() || addr.ip().is_unspecified(),
                    SocketAddrUse::TcpConnect
                    | SocketAddrUse::UdpConnect
                    | SocketAddrUse::UdpOutgoingDatagram => true,
                }
            })
        }),
        loopback: Arc::clone(&template.loopback),
        allowed_network_uses: sockets::AllowedNetworkUses {
            ip_name_lookup: template.local_resources.allow_ip_name_lookup,
            ..Default::default()
        },
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
    let mut shared_ctx = SharedCtx::new(active_ctx);

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

async fn new_ephemeral_store(
    call: &EphemeralLinkedCall,
) -> anyhow::Result<wasmtime::Store<SharedCtx>> {
    let mut component_ids = call.linked_component_ids.clone();
    component_ids.push(call.active_component_id.clone());
    component_ids.sort();
    component_ids.dedup();
    resolve_component_volume_mounts_in_map(&call.components, &component_ids).await?;

    let (active_metadata, linked_metadata) = {
        let components = call.components.read().await;
        let active = components
            .get(&call.active_component_id)
            .with_context(|| {
                format!(
                    "ephemeral linked component '{}' not found",
                    call.active_component_id
                )
            })?
            .metadata
            .clone();
        let linked = call
            .linked_component_ids
            .iter()
            .map(|component_id| {
                components
                    .get(component_id)
                    .with_context(|| format!("linked component '{component_id}' not found"))
                    .map(|component| component.metadata.clone())
            })
            .collect::<wasmtime::Result<Vec<_>>>()?;
        (active, linked)
    };

    #[cfg(not(feature = "wasi-tls"))]
    let active = component_ctx_template_from_metadata(&active_metadata);
    #[cfg(feature = "wasi-tls")]
    let active =
        component_ctx_template_from_metadata_with_tls(&active_metadata, call.tls_provider.clone());

    #[cfg(not(feature = "wasi-tls"))]
    let linked = linked_metadata
        .iter()
        .map(component_ctx_template_from_metadata)
        .collect::<Vec<_>>();
    #[cfg(feature = "wasi-tls")]
    let linked = linked_metadata
        .iter()
        .map(|metadata| {
            component_ctx_template_from_metadata_with_tls(metadata, call.tls_provider.clone())
        })
        .collect::<Vec<_>>();

    let linked_instances = {
        let mut components = call.components.write().await;
        call.linked_component_ids
            .iter()
            .map(|component_id| {
                let component = components.get_mut(component_id).ok_or_else(|| {
                    wasmtime::format_err!("linked component '{component_id}' not found")
                })?;
                component
                    .pre_instantiate()
                    .map(|pre| (component_id.clone(), pre))
            })
            .collect::<wasmtime::Result<Vec<_>>>()
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to pre-instantiate linked components for ephemeral call: {e}"
                )
            })?
    };

    new_store_from_templates(
        &call.engine,
        call.http_handler.clone(),
        &active,
        &linked,
        &linked_instances,
        false,
    )
    .await
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

    let relocated = ready_rx
        .await
        .map_err(|_| wasmtime::format_err!("ephemeral store dropped before producing results"))??;

    // Results are in hand; the task must keep running to feed any result streams,
    // so detach it (cancellation past this point is handled by the result-stream
    // consumers closing their pump channels).
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

/// Run a plain-value async linked call in a short-lived store that is dropped
/// (reclaiming its core-instance slots) as soon as the call returns.
async fn invoke_ephemeral_plain(
    params: &[Val],
    results: &mut [Val],
    inv: &LinkedExportInvocation,
    ephemeral_call: &EphemeralLinkedCall,
) -> wasmtime::Result<()> {
    let mut store = new_ephemeral_store(ephemeral_call)
        .await
        .map_err(|e| wasmtime::format_err!("{e:#}"))?;

    let params_buf = params.to_vec();
    let mut results_buf = vec![Val::Bool(false); results.len()];
    let call_import_name = inv.import_name.clone();
    let call_export_name = inv.export_name.clone();
    let callee_pre = inv.pre.clone();
    let func_idx = inv.func_idx;

    trace!(
        name = %inv.import_name,
        fn_name = %inv.export_name,
        ?params,
        "invoking ephemeral dynamic export"
    );

    let mut task = AbortOnDrop(tokio::task::spawn(async move {
        let instance = callee_pre.instantiate_async(&mut store).await?;
        store
            .run_concurrent(async move |accessor| {
                let func = accessor.with(|mut access| -> wasmtime::Result<_> {
                    instance.get_func(&mut access, func_idx).with_context(|| {
                        format!(
                            "function not found for linked import {call_import_name}.{call_export_name}"
                        )
                    })
                })?;
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
            .map_err(|e| wasmtime::format_err!("{e:#}"))?
    }));
    let call_result = (&mut task.0)
        .await
        .map_err(|e| wasmtime::format_err!("ephemeral linked call task failed: {e}"));
    let call_result = call_result??;

    for (i, v) in call_result.into_iter().enumerate() {
        *results.get_mut(i).context("result index out of bounds")? = v;
    }

    trace!(
        name = %inv.import_name,
        fn_name = %inv.export_name,
        ?results,
        "invoked ephemeral dynamic export"
    );

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
