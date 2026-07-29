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
    plugin::{HostPlugin, WitInterfaces},
    wit::{WitInterface, WitWorld},
};

mod bindings {
    wasmtime::component::bindgen!({
        world: "config",
        imports: { default: async | trappable | tracing },
    });
}

pub(crate) const WASI_CONFIG_ID: &str = "wasi-config";

type ConfigMap = HashMap<Arc<str>, HashMap<String, String>>;

/// WASI configuration plugin that provides access to configuration data.
///
/// This plugin implements the WASI config interface, allowing components to
/// retrieve configuration values and environment variables at runtime. Each
/// component gets an isolated view: the workload-scoped `wasi:config`
/// interface config layered with its own `LocalResources.config` (and, with
/// `copy_environment`, its `LocalResources.environment`).
///
/// Construct via [`DynamicConfig::builder`] (or [`Default::default`]) so new
/// optional knobs land without breaking callers.
///
/// # Examples
///
/// ```
/// use wash_runtime::plugin::wasi_config::DynamicConfig;
///
/// let _plugin = DynamicConfig::builder().copy_environment(true).build();
/// ```
#[derive(Clone, Default, bon::Builder)]
pub struct DynamicConfig {
    /// When `true`, the component's `LocalResources.environment` is merged
    /// into the per-component `wasi:config/store` view at bind time,
    /// surfacing workload env vars to components as both env vars and
    /// config entries without further plumbing.
    #[builder(default)]
    copy_environment: bool,
    /// Per-component-id key-value store. Always starts empty; not part of
    /// the builder surface.
    #[builder(skip)]
    config: Arc<RwLock<ConfigMap>>,
}

/// Link the `wasi:config/store` host functions onto a host component plugin's
/// linker. The values served come from whatever [`DynamicConfig`] view is
/// registered on that store's `Ctx` under [`WASI_CONFIG_ID`] — for a plugin
/// store, the environment-backed view from [`env_view`].
#[cfg(feature = "host-component-plugins")]
pub(crate) fn add_store_to_linker(
    linker: &mut wasmtime::component::Linker<SharedCtx>,
) -> anyhow::Result<()> {
    bindings::wasi::config::store::add_to_linker::<_, SharedCtx>(linker, extract_active_ctx)?;
    Ok(())
}

/// An environment-backed `wasi:config` view for a host component plugin store,
/// served under `view_id` (the plugin's id, which is also the plugin store's
/// `component_id`).
///
/// A host component plugin is the trust peer of the native plugins compiled
/// into the host, which read `std::env` freely — so the view is a snapshot of
/// the **full host process environment**, taken when the plugin store is built
/// (fresh on each supervised restart). Each variable is served under its raw
/// name and, when different, under a kebab-case alias (`ETCD_ENDPOINTS` →
/// `etcd-endpoints`) so components can use conventional `wasi:config` keys.
#[cfg(feature = "host-component-plugins")]
pub(crate) fn env_view(view_id: &str) -> DynamicConfig {
    let mut view = HashMap::new();
    for (name, value) in std::env::vars() {
        let kebab = name.to_ascii_lowercase().replace(['_', '.'], "-");
        if kebab != name {
            view.insert(kebab, value.clone());
        }
        view.insert(name, value);
    }
    let mut config = ConfigMap::new();
    config.insert(Arc::from(view_id), view);
    DynamicConfig {
        copy_environment: false,
        config: Arc::new(RwLock::new(config)),
    }
}

impl<'a> bindings::wasi::config::store::Host for ActiveCtx<'a> {
    #[instrument(name = "wasi.config.get", skip(self))]
    async fn get(
        &mut self,
        key: String,
    ) -> wasmtime::Result<Result<Option<String>, bindings::wasi::config::store::Error>> {
        let plugin = self.try_get_plugin::<DynamicConfig>(WASI_CONFIG_ID)?;

        let config_guard = plugin.config.read().await;
        config_guard
            .get(&*self.component_id)
            .and_then(|map| map.get(&key).cloned())
            .map_or(Ok(Ok(None)), |v| Ok(Ok(Some(v))))
    }

    #[instrument(name = "wasi.config.get_all", skip(self))]
    async fn get_all(
        &mut self,
    ) -> wasmtime::Result<Result<Vec<(String, String)>, bindings::wasi::config::store::Error>> {
        let plugin = self.try_get_plugin::<DynamicConfig>(WASI_CONFIG_ID)?;

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
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // Find the "wasi:config/store" interface, if present
        let Some(interface) = interfaces.get("wasi", "config", &["store"]) else {
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

        // Per-component view, later wins on key conflicts: interface
        // config, then `LocalResources.config`, then (with
        // `copy_environment`) the environment.
        let component_config = {
            let mut config_map = interface.config.clone();

            for (key, value) in component_handle.local_resources().config.iter() {
                config_map.insert(key.clone(), value.clone());
            }

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
