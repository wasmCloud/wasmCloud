//! Runtime configuration plugin for WebAssembly components.
//!
//! This plugin implements the `wasi:config/store@0.2.0-rc.1` interface,
//! providing components with access to configuration data and environment
//! variables at runtime. It allows components to retrieve configuration
//! values without requiring them to be compiled into the component.
//!
//! # Features
//!
//! - Access to environment variables
//! - Configuration key-value pairs
//! - Runtime configuration updates
//! - Component isolation of configuration data
//!
//! # Usage
//!
//! Components can use this plugin through the standard WASI config interface
//! to retrieve configuration values that are set by the host environment.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::RwLock;
use tracing::instrument;

use crate::{
    engine::{
        ctx::{ActiveCtx, SharedCtx, extract_active_ctx},
        workload::WorkloadItem,
    },
    plugin::HostPlugin,
    wit::{WitInterface, WitWorld},
};

mod bindings {
    wasmtime::component::bindgen!({
        world: "config",
        imports: { default: async | trappable | tracing },
    });
}

const WASI_CONFIG_ID: &str = "wasi-config";

type ConfigMap = HashMap<Arc<str>, HashMap<String, String>>;

/// WASI configuration plugin that provides access to configuration data.
///
/// This plugin implements the WASI config interface, allowing components to
/// retrieve configuration values and environment variables at runtime. Each
/// component gets isolated access to its own configuration scope.
#[derive(Clone, Default)]
pub struct DynamicConfig {
    copy_environment: bool,
    /// A map of configuration from component id to key-value pairs
    config: Arc<RwLock<ConfigMap>>,
}

impl DynamicConfig {
    pub fn new(copy_environment: bool) -> Self {
        Self {
            copy_environment,
            ..Default::default()
        }
    }
}

impl<'a> bindings::wasi::config::store::Host for ActiveCtx<'a> {
    #[instrument(skip(self))]
    async fn get(
        &mut self,
        key: String,
    ) -> wasmtime::Result<Result<Option<String>, bindings::wasi::config::store::Error>> {
        let Some(plugin) = self.get_plugin::<DynamicConfig>(WASI_CONFIG_ID) else {
            return Ok(Ok(None));
        };
        let config_guard = plugin.config.read().await;
        config_guard
            .get(&*self.component_id)
            .and_then(|map| map.get(&key).cloned())
            .map_or(Ok(Ok(None)), |v| Ok(Ok(Some(v))))
    }

    #[instrument(skip(self))]
    async fn get_all(
        &mut self,
    ) -> wasmtime::Result<Result<Vec<(String, String)>, bindings::wasi::config::store::Error>> {
        let Some(plugin) = self.get_plugin::<DynamicConfig>(WASI_CONFIG_ID) else {
            return Ok(Ok(vec![]));
        };
        let config_guard = plugin.config.read().await;
        let entries = config_guard
            .get(&*self.component_id)
            .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        Ok(Ok(entries))
    }
}

#[async_trait::async_trait]
impl HostPlugin for DynamicConfig {
    fn id(&self) -> &'static str {
        WASI_CONFIG_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("wasi:config/store@0.2.0-rc.1")]),
            exports: HashSet::new(),
        }
    }
    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Find the "wasi:config/store" interface, if present
        let Some(interface) = interfaces.iter().find(|i| {
            i.namespace == "wasi" && i.package == "config" && i.interfaces.contains("store")
        }) else {
            // Log a warning if the requested interfaces are not wasi:config/store
            tracing::warn!(
                "WasiConfig plugin requested for non-wasi:config/store interface(s): {:?}",
                interfaces
            );
            return Ok(());
        };

        // Add `wasi:config/store` to the workload's linker
        bindings::wasi::config::store::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;

        let component_config = {
            let mut config_map = interface.config.clone();

            if self.copy_environment {
                for (key, value) in component_handle.local_resources().environment.iter() {
                    config_map.insert(key.into(), value.into());
                }
            }

            config_map
        };

        // Store the configuration for lookups later
        self.config
            .write()
            .await
            .insert(Arc::from(component_handle.id()), component_config);

        Ok(())
    }
}
