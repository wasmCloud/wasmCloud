//! wRPC plugin for bridging arbitrary WIT interfaces over NATS.
//!
//! This plugin allows components to call remote services (imports) and be called
//! remotely (exports) via the wRPC protocol over NATS transport. Components declare
//! which interfaces to bridge using a `wrpc:name` config key in their interface
//! declarations.
//!
//! ## Architecture
//!
//! - **Imports**: Polyfilled via `wrpc_runtime_wasmtime::link_instance` which handles
//!   all encoding/decoding automatically. A `RoutingInvoker` on the store's `WrpcView`
//!   maps WIT instance names to `wrpc_transport_nats::Client` instances.
//! - **Exports**: Served via `wrpc_transport_nats::Client::serve()` with handler
//!   tasks that delegate to `wrpc_runtime_wasmtime::call`.
//! - **SharedCtx integration**: `WrpcView` on `SharedCtx` provides the `RoutingInvoker`
//!   for import polyfilling and resource table access for export serving.
//!
//! ## NATS Subject Pattern
//!
//! Each routing key maps to a wRPC NATS client with prefix `{plugin_prefix}.{routing_key}`.
//! wRPC internally appends `.wrpc.0.0.1.{instance}.{func}` to form the final subject.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info, instrument};
use wasmtime::component::types::ComponentItem;

use crate::engine::workload::{ResolvedWorkload, WorkloadItem};
use crate::plugin::{HostPlugin, WorkloadTracker};
use crate::wit::{WitInterface, WitWorld};

pub const PLUGIN_WRPC_ID: &str = "wrpc";

pub(crate) mod codec;
mod invoke;
mod serve;

/// Metadata about an export function to be served via wRPC.
#[derive(Clone)]
struct ExportInfo {
    instance_name: String,
    func_name: String,
    param_types: Vec<wasmtime::component::Type>,
    result_types: Vec<wasmtime::component::Type>,
}

/// Per-component tracking data for the wRPC plugin.
struct ComponentData {
    /// Export functions grouped by routing key.
    exports: HashMap<String, Vec<ExportInfo>>,
    /// Cancellation token for stopping export serving tasks.
    cancel_token: tokio_util::sync::CancellationToken,
}

/// Plugin that bridges WIT interfaces over wRPC/NATS.
///
/// Components declare which interfaces to bridge by setting `wrpc:name` in the
/// interface config. The value is a routing key that determines the NATS subject
/// prefix for that interface.
pub struct WrpcPlugin {
    nats_client: Arc<async_nats::Client>,
    prefix: String,
    tracker: Arc<RwLock<WorkloadTracker<(), ComponentData>>>,
}

impl WrpcPlugin {
    /// Create a new wRPC plugin.
    ///
    /// # Arguments
    /// * `nats_client` - The NATS client to use for wRPC transport
    /// * `prefix` - Base prefix for NATS subjects (e.g., "wash")
    pub fn new(nats_client: Arc<async_nats::Client>, prefix: impl Into<String>) -> Self {
        Self {
            nats_client,
            prefix: prefix.into(),
            tracker: Arc::new(RwLock::new(WorkloadTracker::default())),
        }
    }
}

#[async_trait::async_trait]
impl HostPlugin for WrpcPlugin {
    fn id(&self) -> &'static str {
        PLUGIN_WRPC_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld::default()
    }

    fn can_handle(&self, interface: &WitInterface) -> bool {
        info!(
            interface = %interface,
            has_wrpc_name = interface.config.contains_key("wrpc:name"),
            "checking if wrpc plugin can handle interface"
        );
        interface.config.contains_key("wrpc:name")
    }

    #[instrument(skip_all, fields(plugin = PLUGIN_WRPC_ID))]
    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        info!("binding imports");
        // Bind import functions (component calls out via wrpc)
        invoke::bind_imports(&self.nats_client, &self.prefix, item, &interfaces).await?;

        info!("collecting exports");
        // Collect export function metadata for on_workload_resolved
        let exports = collect_exports(item, &interfaces)?;

        if !exports.is_empty() {
            let WorkloadItem::Component(component_handle) = item else {
                anyhow::bail!("wrpc export serving requires a component, not a service");
            };
            info!(
                num_export_interfaces = exports.len(),
                "bound wrpc exports for component"
            );

            self.tracker.write().await.add_component(
                component_handle,
                ComponentData {
                    exports,
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                },
            );
        }

        Ok(())
    }

    #[instrument(skip_all, fields(plugin = PLUGIN_WRPC_ID, component_id = %component_id))]
    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        debug!("starting wrpc export serving for resolved workload");
        let (cancel_token, exports) = {
            let lock = self.tracker.read().await;
            match lock.get_component_data(component_id) {
                Some(data) => (data.cancel_token.clone(), data.exports.clone()),
                None => return Ok(()),
            }
        };

        if exports.is_empty() {
            debug!(
                component_id = %component_id,
                "no wrpc exports to serve for this component"
            );
            return Ok(());
        }

        serve::serve_exports(
            &self.nats_client,
            &self.prefix,
            workload,
            component_id,
            &exports,
            cancel_token,
        )
        .await?;

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        self.tracker
            .write()
            .await
            .remove_workload_with_cleanup(
                workload_id,
                |_| async {},
                |data: ComponentData| async move {
                    data.cancel_token.cancel();
                },
            )
            .await;

        Ok(())
    }
}

/// Collect export function metadata from the component for interfaces that have `wrpc:name`.
/// Returns a map from routing_key → list of export functions.
fn collect_exports(
    item: &WorkloadItem<'_>,
    interfaces: &HashSet<WitInterface>,
) -> anyhow::Result<HashMap<String, Vec<ExportInfo>>> {
    let component = item.component().clone();
    let engine = component.engine();
    let component_type = component.component_type();
    let mut exports_by_key: HashMap<String, Vec<ExportInfo>> = HashMap::new();

    for interface in interfaces {
        let routing_key = match interface.config.get("wrpc:name") {
            Some(key) => key.clone(),
            None => continue,
        };
        info!(
            interface = %interface,
            routing_key = %routing_key,
            "collecting exports for interface with wrpc:name"
        );

        // Find this interface in the component's exports
        for (export_name, export_item) in component_type.exports(engine) {
            let ComponentItem::ComponentInstance(instance_ty) = export_item else {
                continue;
            };

            let wit_export = WitInterface::from(export_name);
            if interface.contains(&wit_export) {
                info!(
                    export_name = %export_name,
                    routing_key = %routing_key,
                    "interface matches component export, collecting functions"
                );
            } else {
                info!(
                    export_name = %export_name,
                    routing_key = %routing_key,
                    "interface does not match component export, skipping"
                );
                continue;
            }

            info!(
                export_name= %export_name,
                routing_key = %routing_key,
                "collecting wrpc export functions"
            );

            for (func_name, item_ty) in instance_ty.exports(engine) {
                let ComponentItem::ComponentFunc(func_ty) = item_ty else {
                    continue;
                };

                let param_types: Vec<_> = func_ty.params().map(|(_, ty)| ty).collect();
                let result_types: Vec<_> = func_ty.results().collect();
                exports_by_key
                    .entry(routing_key.clone())
                    .or_default()
                    .push(ExportInfo {
                        instance_name: export_name.into(),
                        func_name: func_name.to_string(),
                        param_types,
                        result_types,
                    });
            }
        }
    }

    Ok(exports_by_key)
}
