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
    collections::{BTreeMap, HashSet},
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

/// One component's `wasi:config/store` view. Ordered, so `get-all` hands a
/// guest the same sequence on every call and every start.
type ComponentConfig = BTreeMap<String, String>;

/// Every bound component's view, keyed by workload then by component, so a
/// workload's configuration leaves with it — see
/// [`HostPlugin::on_workload_unbind`], which has only a workload id to work
/// from. Matches [`crate::plugin::wasmcloud_secrets`]'s shape.
type ConfigMap = BTreeMap<Arc<str>, BTreeMap<Arc<str>, ComponentConfig>>;

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
    /// Each bound component's view, by workload then component (see
    /// [`ConfigMap`]). Always starts empty; not part of the builder surface.
    #[builder(skip)]
    config: Arc<RwLock<ConfigMap>>,
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
            .get(&*self.workload_id)
            .and_then(|components| components.get(&*self.component_id))
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
            .get(&*self.workload_id)
            .and_then(|components| components.get(&*self.component_id))
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
        // This plugin serves one store per component
        // (`supports_named_instances` is false), so every `wasi:config` entry
        // the workload declared feeds that one store.
        let entries = interfaces.matching("wasi", "config", &["store"]);
        if entries.is_empty() {
            // Log a warning if the requested interfaces are not wasi:config/store
            tracing::warn!(
                "WasiConfig plugin requested for non-wasi:config/store interface(s): {:?}",
                interfaces
            );
            return Ok(());
        }

        // Add `wasi:config/store` to the workload's linker
        bindings::wasi::config::store::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;

        // Per-component view, later wins on key conflicts: the workload's
        // interface entries in order, then `LocalResources.config`, then (with
        // `copy_environment`) the environment.
        let component_config = {
            let mut config_map = ComponentConfig::new();

            for entry in entries {
                for (key, value) in entry.config.iter() {
                    if let Some(previous) = config_map.insert(key.clone(), value.clone())
                        && &previous != value
                    {
                        tracing::warn!(
                            key,
                            component_id = component_handle.id(),
                            "two wasi:config entries set the same key to different values; \
                             the workload's entries are one store, so the later entry wins"
                        );
                    }
                }
            }

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
            .entry(Arc::from(component_handle.workload_id()))
            .or_default()
            .insert(Arc::from(component_handle.id()), component_config);

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // The engine calls this once per component bound to this plugin, each
        // carrying the same workload id, and a workload's components stop
        // together — so dropping them on the first call is correct and the rest
        // are no-ops. Component ids are fresh per start, so without this a
        // restart leaves the old ids behind for as long as the host runs,
        // holding whatever `secretFrom` resolved into them.
        let mut config = self.config.write().await;
        let Some(components) = config.get_mut(workload_id) else {
            return Ok(());
        };
        // A host component plugin's own bind-time config is filed under its
        // plugin id as both ids (`component_host::link_native_imports`), and
        // workload ids come from the caller. A workload that happens to carry a
        // plugin's id stops without taking that plugin's config with it.
        components.retain(|component_id, _| component_id.as_ref() == workload_id);
        if components.is_empty() {
            config.remove(workload_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{
        InstancePolicy, workload::UnresolvedWorkload, workload::WorkloadComponent,
    };
    use crate::types::LocalResources;
    use std::collections::HashMap;
    use wasmtime::component::{Component, Linker};

    fn config_importer(name: &str, config: &[(&str, &str)]) -> WorkloadComponent {
        component_from_wat(
            name,
            r#"(component (import "wasi:config/store@0.2.0-rc.1" (instance)))"#,
            config,
        )
    }

    fn component_from_wat(name: &str, wat: &str, config: &[(&str, &str)]) -> WorkloadComponent {
        let engine = wasmtime::Engine::default();
        let linker = Linker::new(&engine);
        let wasm = wat::parse_str(wat).expect("failed to parse WAT");
        let component = Component::new(&engine, &wasm).expect("failed to compile");
        let local_resources = LocalResources {
            config: config
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        };
        WorkloadComponent::new(
            "workload-config",
            "test-workload",
            "test-namespace",
            name,
            component,
            linker,
            Vec::new(),
            local_resources,
            Arc::default(),
            InstancePolicy::Ephemeral,
        )
    }

    async fn bind(
        components: Vec<WorkloadComponent>,
        host_interfaces: Vec<WitInterface>,
    ) -> (Arc<DynamicConfig>, anyhow::Result<()>) {
        let plugin = Arc::new(DynamicConfig::default());
        let plugins: std::collections::HashMap<&'static str, Arc<dyn HostPlugin>> =
            std::collections::HashMap::from([(
                plugin.id(),
                Arc::clone(&plugin) as Arc<dyn HostPlugin>,
            )]);
        let mut workload = UnresolvedWorkload::new(
            "workload-config",
            "test-workload",
            "test-namespace",
            None,
            components,
            host_interfaces,
        );
        let res = workload
            .bind_plugins(&plugins, &crate::plugin::PluginBindings::new())
            .await
            .map(|_| ());
        (plugin, res)
    }

    const WORKLOAD: &str = "workload-config";

    /// One component's view out of the workload it was bound under.
    fn view<'a>(stored: &'a ConfigMap, component_id: &str) -> &'a ComponentConfig {
        stored
            .get(WORKLOAD)
            .and_then(|components| components.get(component_id))
            .unwrap_or_else(|| panic!("no config view for component {component_id}"))
    }

    fn shared(entry: &str) -> WitInterface {
        let mut interface = WitInterface::from(entry);
        interface.config = HashMap::from([("shared".to_string(), "yes".to_string())]);
        interface
    }

    #[tokio::test]
    async fn every_component_that_imports_wasi_config_gets_its_own_view() {
        let a = config_importer("a", &[("own", "a")]);
        let b = config_importer("b", &[("own", "b")]);
        let a_id = a.id().to_string();
        let b_id = b.id().to_string();

        let (plugin, res) = bind(vec![a, b], vec![shared("wasi:config/store@0.2.0-rc.1")]).await;
        res.expect("bind should succeed");

        let stored = plugin.config.read().await;
        for (id, own) in [(&a_id, "a"), (&b_id, "b")] {
            let view = view(&stored, id);
            assert_eq!(view.get("shared").map(String::as_str), Some("yes"));
            assert_eq!(view.get("own").map(String::as_str), Some(own));
        }
    }

    /// A workload may spread one binding over several entries, and they are one
    /// store: every component reads all of them.
    #[tokio::test]
    async fn every_matching_entry_lands_in_one_view() {
        let mut package_only = WitInterface::from("wasi:config");
        package_only.config = HashMap::from([("extra".to_string(), "1".to_string())]);

        // Repeated because the order under test comes from a `HashSet`: one
        // run proves nothing.
        for _ in 0..16 {
            let a = config_importer("a", &[("own", "a")]);
            let b = config_importer("b", &[("own", "b")]);
            let a_id = a.id().to_string();
            let b_id = b.id().to_string();

            let (plugin, res) = bind(
                vec![a, b],
                vec![shared("wasi:config/store@0.2.0-rc.1"), package_only.clone()],
            )
            .await;
            res.expect("bind should succeed");

            let stored = plugin.config.read().await;
            for (id, own) in [(&a_id, "a"), (&b_id, "b")] {
                let view = view(&stored, id);
                assert_eq!(view.get("shared").map(String::as_str), Some("yes"));
                assert_eq!(view.get("extra").map(String::as_str), Some("1"));
                assert_eq!(view.get("own").map(String::as_str), Some(own));
            }
        }
    }

    /// An entry naming the package alone covers every interface in it, the rule
    /// it was matched to this plugin under, so it configures the component and
    /// puts `wasi:config/store` in its linker.
    #[tokio::test]
    async fn a_package_only_entry_is_still_a_config_entry() {
        let a = config_importer("a", &[("own", "a")]);
        let a_id = a.id().to_string();

        let (plugin, res) = bind(vec![a], vec![shared("wasi:config")]).await;
        res.expect("bind should succeed");

        let stored = plugin.config.read().await;
        let view = view(&stored, &a_id);
        assert_eq!(view.get("shared").map(String::as_str), Some("yes"));
        assert_eq!(view.get("own").map(String::as_str), Some("a"));
    }

    /// A component that also imports a sibling's export is bound the same way:
    /// intra-workload linking is a separate question from host config.
    #[tokio::test]
    async fn a_linked_component_gets_its_own_view() {
        let a = component_from_wat(
            "a",
            r#"(component
                 (import "wasi:config/store@0.2.0-rc.1" (instance))
                 (import "test:probe/marker@0.1.0" (instance)))"#,
            &[("own", "a")],
        );
        let b = component_from_wat(
            "b",
            r#"(component
                 (import "wasi:config/store@0.2.0-rc.1" (instance))
                 (instance $m)
                 (export "test:probe/marker@0.1.0" (instance $m)))"#,
            &[("own", "b")],
        );
        let a_id = a.id().to_string();
        let b_id = b.id().to_string();

        let (plugin, res) = bind(vec![a, b], vec![shared("wasi:config/store@0.2.0-rc.1")]).await;
        res.expect("bind should succeed");

        let stored = plugin.config.read().await;
        for (id, own) in [(&a_id, "a"), (&b_id, "b")] {
            assert_eq!(view(&stored, id).get("own").map(String::as_str), Some(own));
        }
    }

    /// A workload's configuration leaves with the workload. Component ids are
    /// fresh per start, so what a stopped workload leaves behind is never read
    /// again — and it holds whatever `secretFrom` resolved into it.
    #[tokio::test]
    async fn unbinding_a_workload_drops_its_config() {
        let a = config_importer("a", &[("own", "a")]);
        let a_id = a.id().to_string();

        let (plugin, res) = bind(vec![a], vec![shared("wasi:config/store@0.2.0-rc.1")]).await;
        res.expect("bind should succeed");
        {
            let stored = plugin.config.read().await;
            assert_eq!(
                view(&stored, &a_id).get("own").map(String::as_str),
                Some("a")
            );
        }

        // Another workload's configuration is untouched by the unbind.
        let empty = HashSet::new();
        plugin
            .on_workload_unbind("some-other-workload", WitInterfaces::new(&empty))
            .await
            .expect("unbinding an unrelated workload is a no-op");
        assert!(plugin.config.read().await.contains_key(WORKLOAD));

        plugin
            .on_workload_unbind(WORKLOAD, WitInterfaces::new(&empty))
            .await
            .expect("unbind should succeed");
        assert!(
            plugin.config.read().await.is_empty(),
            "the workload's config outlived it"
        );
    }

    /// A host component plugin's own config is filed under its plugin id as
    /// both the workload and the component id, and workload ids come from the
    /// caller: a workload carrying that id stops without taking it away.
    #[tokio::test]
    async fn a_plugin_keeps_its_own_config_when_a_workload_shares_its_id() {
        let plugin = DynamicConfig::default();
        let id: Arc<str> = Arc::from("acme-kv");
        {
            let mut config = plugin.config.write().await;
            let components = config.entry(Arc::clone(&id)).or_default();
            components.insert(
                Arc::clone(&id),
                ComponentConfig::from([("who".to_string(), "the plugin".to_string())]),
            );
            components.insert(
                Arc::from("a-workload-component"),
                ComponentConfig::from([("who".to_string(), "the workload".to_string())]),
            );
        }

        let empty = HashSet::new();
        plugin
            .on_workload_unbind(&id, WitInterfaces::new(&empty))
            .await
            .expect("unbind should succeed");

        let config = plugin.config.read().await;
        let components = config.get(&id).expect("the plugin's own config survives");
        assert!(components.contains_key(&id));
        assert!(!components.contains_key("a-workload-component"));
    }
}
