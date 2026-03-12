//! # WASI Logging Plugin
//!
//! This module routes logging calls from WASI components to the host's tracing
//! system. It implements the `wasi:logging/logging` interface, allowing
//! components to log messages at various levels (trace, debug, info, warn,
//! error, critical).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};
use wasmtime::bail;

const PLUGIN_LOGGING_ID: &str = "wasi-logging";

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "logging",
        imports: { default: async | trappable | tracing },
    });
}

use bindings::wasi::logging::logging::Level;
use tokio::sync::RwLock;

type ComponentMap = Arc<RwLock<HashMap<String, ComponentInfo>>>;

#[derive(Default)]
pub struct TracingLogger {
    components: ComponentMap,
}

struct ComponentInfo {
    workload_name: String,
    workload_namespace: String,
    component_id: String,
}

impl<'a> bindings::wasi::logging::logging::Host for ActiveCtx<'a> {
    async fn log(
        &mut self,
        level: Level,
        context: String,
        message: String,
    ) -> wasmtime::Result<()> {
        let Some(plugin) = self.get_plugin::<TracingLogger>(PLUGIN_LOGGING_ID) else {
            bail!("TracingLogger plugin not found in context");
        };

        let workloads = plugin.components.read().await;
        let Some(ComponentInfo {
            workload_name,
            workload_namespace,
            component_id,
        }) = workloads.get(&self.component_id.to_string())
        else {
            bail!("Component not found in TracingLogger plugin");
        };
        match level {
            Level::Trace => {
                tracing::trace!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Debug => {
                tracing::debug!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Info => {
                tracing::info!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Warn => {
                tracing::warn!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Error => {
                tracing::error!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Critical => {
                tracing::error!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
        };

        Ok(())
    }
}

#[async_trait::async_trait]
impl HostPlugin for TracingLogger {
    fn id(&self) -> &'static str {
        PLUGIN_LOGGING_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("wasi:logging/logging")]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: std::collections::HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        // Ensure exactly one interface: "wasi:logging/logging"
        let has_logging = interfaces
            .iter()
            .any(|i| i.namespace == "wasi" && i.package == "logging");

        if !has_logging {
            tracing::warn!(
                "TracingLogger plugin requested for non-wasi:logging interface(s): {:?}",
                interfaces
            );
            return Ok(());
        }

        // Add `wasi:logging/logging` to the workload's linker
        bindings::wasi::logging::logging::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;

        self.components.write().await.insert(
            component_handle.id().to_string(),
            ComponentInfo {
                workload_name: component_handle.workload_name().to_string(),
                workload_namespace: component_handle.workload_namespace().to_string(),
                component_id: component_handle.id().to_string(),
            },
        );

        Ok(())
    }
}
