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
use std::time::Duration;

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
}

fn type_is_ephemeral_safe(ty: &Type) -> bool {
    !carries_cross_store_handle(ty)
}

pub(crate) fn func_is_ephemeral_safe(func_ty: &ComponentFunc) -> bool {
    func_ty.params().all(|(_, ty)| type_is_ephemeral_safe(&ty))
        && func_ty.results().all(|ty| type_is_ephemeral_safe(&ty))
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
        invoke_ephemeral_linked_export(params, results, inv, ephemeral_call).await
    } else {
        invoke_shared_store_linked_export(accessor, params, results, inv).await
    }
}

/// Run a plain-value async linked call in a short-lived store that is dropped
/// (reclaiming its core-instance slots) as soon as the call returns.
async fn invoke_ephemeral_linked_export(
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
    let call_pre = inv.pre.clone();
    let func_idx = inv.func_idx;

    trace!(
        name = %inv.import_name,
        fn_name = %inv.export_name,
        ?params,
        "invoking ephemeral dynamic export"
    );

    let call_result = tokio::task::spawn(async move {
        let instance = call_pre.instantiate_async(&mut store).await?;
        store
            .run_concurrent(async move |accessor| {
                let func = accessor.with(|mut access| -> wasmtime::Result<_> {
                    instance.get_func(&mut access, func_idx).with_context(|| {
                        format!(
                            "function not found for linked import {call_import_name}.{call_export_name}"
                        )
                    })
                })?;
                const CALL_TIMEOUT: Duration = Duration::from_secs(600);
                timeout(
                    CALL_TIMEOUT,
                    func.call_concurrent(accessor, &params_buf, &mut results_buf),
                )
                .await
                .map_err(|e| {
                    wasmtime::format_err!("function call timed out after 600 seconds: {e}")
                })??;
                Ok::<Vec<Val>, wasmtime::Error>(results_buf)
            })
            .await
            .map_err(|e| wasmtime::format_err!("{e:#}"))?
    })
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

        const CALL_TIMEOUT: Duration = Duration::from_secs(30);
        timeout(
            CALL_TIMEOUT,
            func.call_async(&mut store, &params_buf, &mut results_buf),
        )
        .await
        .map_err(|e| wasmtime::format_err!("function call timed out after 30 seconds: {e}"))??;

        lift_results(store, results_buf, results)?;
        trace!(name = %inv.import_name, fn_name = %inv.export_name, "invoked dynamic export");
        Ok(())
    }
    .await
}
