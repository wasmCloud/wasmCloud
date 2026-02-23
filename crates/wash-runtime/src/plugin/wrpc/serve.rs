//! Export bridging: serve component exports via wRPC/NATS so remote callers can invoke them.
//!
//! For each component export that has a `wrpc:name` config key, this module subscribes
//! to wRPC NATS subjects and spawns tasks that use `wrpc_runtime_wasmtime::call` to
//! handle the full decode → call → encode cycle.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use futures::stream::StreamExt;
use tracing::{debug, instrument, trace, warn};
use wasmtime::component::{Func, Type};
use wrpc_transport::Serve as _;

use super::ExportInfo;
use crate::engine::workload::ResolvedWorkload;

/// Serve exported functions for a component via wRPC NATS.
///
/// For each export interface with a `wrpc:name` config key, subscribes to wRPC
/// NATS subjects and spawns handler tasks that delegate to
/// `wrpc_runtime_wasmtime::call`.
#[instrument(skip_all, fields(component_id = %component_id))]
pub(super) async fn serve_exports(
    nats_client: &Arc<async_nats::Client>,
    prefix: &str,
    workload: &ResolvedWorkload,
    component_id: &str,
    exports: &HashMap<String, Vec<ExportInfo>>,
    cancel_token: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    debug!(
        component_id = %component_id,
        num_export_interfaces = exports.len(),
        "setting up wrpc export serving for component"
    );
    for (routing_key, export_infos) in exports {
        let wrpc_prefix = format!("{prefix}.{routing_key}");

        let wrpc_client = wrpc_transport_nats::Client::new(
            nats_client.clone(),
            Arc::from(wrpc_prefix.as_str()),
            None,
        )
        .await
        .context("failed to create wrpc NATS client for serving")?;

        debug!(
            routing_key = %routing_key,
            num_functions = export_infos.len(),
            "setting up wrpc export subscriptions"
        );

        for export_info in export_infos {
            let instance_name = export_info.instance_name.clone();
            let func_name = export_info.func_name.clone();
            let param_types = export_info.param_types.clone();
            let result_types = export_info.result_types.clone();

            let invocations = wrpc_client
                .serve(&instance_name, &func_name, [])
                .await
                .with_context(|| {
                    format!("failed to subscribe to wrpc export {instance_name}/{func_name}")
                })?;

            let workload = workload.clone();
            let component_id = component_id.to_string();
            let cancel = cancel_token.clone();
            let instance_name_clone = instance_name.clone();
            let func_name_clone = func_name.clone();

            tokio::spawn(async move {
                let mut invocations = std::pin::pin!(invocations);
                loop {
                    tokio::select! {
                        maybe_invocation = invocations.next() => {
                            let invocation = match maybe_invocation {
                                None => break,
                                Some(Ok(inv)) => inv,
                                Some(Err(e)) => {
                                    warn!(
                                        instance = %instance_name_clone,
                                        func = %func_name_clone,
                                        error = %e,
                                        "wrpc serve stream error"
                                    );
                                    continue;
                                }
                            };

                            if let Err(e) = handle_invocation(
                                invocation,
                                &workload,
                                &component_id,
                                &instance_name_clone,
                                &func_name_clone,
                                &param_types,
                                &result_types,
                            ).await {
                                warn!(
                                    instance = %instance_name_clone,
                                    func = %func_name_clone,
                                    error = %e,
                                    "wrpc export invocation failed"
                                );
                            }
                        }
                        _ = cancel.cancelled() => {
                            debug!(
                                instance = %instance_name_clone,
                                func = %func_name_clone,
                                "wrpc export serving cancelled"
                            );
                            break;
                        }
                    }
                }
            });
        }
    }

    Ok(())
}

/// Handle a single wRPC invocation using `wrpc_runtime_wasmtime::call`.
#[instrument(skip_all, fields(instance = %instance_name, func = %func_name))]
async fn handle_invocation<Tx, Rx>(
    (_cx, tx, rx): (wrpc_transport_nats::NatsContext, Tx, Rx),
    workload: &ResolvedWorkload,
    component_id: &str,
    instance_name: &str,
    func_name: &str,
    param_types: &[Type],
    result_types: &[Type],
) -> anyhow::Result<()>
where
    Tx: tokio::io::AsyncWrite + wrpc_transport::Index<Tx> + Send + Sync + Unpin + 'static,
    Rx: tokio::io::AsyncRead + wrpc_transport::Index<Rx> + Send + Sync + Unpin + 'static,
{
    trace!("handling wrpc invocation");

    // Create a fresh store and instantiate the component
    let mut store = workload
        .new_store(component_id)
        .await
        .context("failed to create store for wrpc export")?;

    let instance_pre = workload
        .instantiate_pre(component_id)
        .await
        .context("failed to get instance pre for wrpc export")?;

    let instance = instance_pre
        .instantiate_async(&mut store)
        .await
        .context("failed to instantiate component for wrpc export")?;

    // Find the exported function
    let func = find_export_func(&instance, &mut store, instance_name, func_name)
        .context("failed to find exported function")?;

    // Delegate the full decode → call → encode cycle to wrpc-runtime-wasmtime
    wrpc_runtime_wasmtime::call(
        &mut store,
        rx,
        tx,
        &[],             // guest_resources: none (no resource bridging)
        &HashMap::new(), // host_resources: none
        param_types.iter(),
        result_types,
        func,
    )
    .await
    .map_err(|e| anyhow::anyhow!("wrpc export call failed: {e}"))?;

    trace!("wrpc invocation handled successfully");
    Ok(())
}

/// Find an exported function by instance and function name within a component instance.
fn find_export_func(
    instance: &wasmtime::component::Instance,
    store: &mut wasmtime::Store<crate::engine::ctx::SharedCtx>,
    instance_name: &str,
    func_name: &str,
) -> anyhow::Result<Func> {
    // First look up the export instance index, then the function within it
    let instance_index = instance
        .get_export_index(&mut *store, None, instance_name)
        .with_context(|| format!("export instance '{instance_name}' not found"))?;
    let func_index = instance
        .get_export_index(&mut *store, Some(&instance_index), func_name)
        .with_context(|| format!("export function '{func_name}' not found in '{instance_name}'"))?;
    instance
        .get_func(&mut *store, func_index)
        .with_context(|| format!("'{instance_name}/{func_name}' is not a function"))
}
