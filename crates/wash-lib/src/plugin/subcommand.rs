wasmtime::component::bindgen!({
    world: "subcommands",
    async: true,
});

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine};
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi_http::WasiHttpCtx;

use super::Data;
use exports::wasmcloud::wash::subcommand::Metadata;

struct InstanceData {
    instance: Subcommands,
    metadata: Metadata,
    store: wasmtime::Store<Data>,
}

pub struct SubcommandRunner {
    engine: Engine,
    plugins: HashMap<String, InstanceData>,
}

impl SubcommandRunner {
    /// Creates a new subcommand runner with no plugins loaded.
    pub fn new() -> anyhow::Result<Self> {
        let mut config = Config::new();
        // Attempt to use caching, but only warn if it fails
        if let Err(e) = config.cache_config_load_default() {
            tracing::warn!(err = ?e, "Failed to load wasm cache");
        }
        config.wasm_component_model(true);
        config.async_support(true);
        let engine = Engine::new(&config)?;
        Ok(Self {
            engine,
            plugins: HashMap::new(),
        })
    }

    /// Create a new runner initialized with the list of plugins provided.
    ///
    /// This function will fail if any of the plugins fail to load. If you want to gracefully handle
    /// errors, use [`add_plugin`](Self::add_plugin) instead.
    pub async fn new_with_plugins(
        plugins: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> anyhow::Result<Self> {
        let mut runner = Self::new()?;
        for plugin in plugins {
            runner.add_plugin(plugin).await?;
        }
        Ok(runner)
    }

    /// Adds a plugin to the runner, returning the metadata for the plugin and otherwise returning
    /// an error if there was a problem loading the plugin. This can happen due to bad instantiation
    /// or if a plugin with the same ID has already been loaded. As such, errors from this function
    /// should be treated as a warning as execution can continue
    pub async fn add_plugin(&mut self, path: impl AsRef<Path>) -> anyhow::Result<Metadata> {
        // We create a bare context here for registration and then update the store with a new context before running
        let ctx = WasiCtxBuilder::new().build();

        let ctx = Data {
            table: wasmtime::component::ResourceTable::default(),
            ctx,
            http: WasiHttpCtx,
        };

        let mut store = wasmtime::Store::new(&self.engine, ctx);

        let component = Component::from_file(&self.engine, path)?;
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::command::add_to_linker(&mut linker)?;
        wasmtime_wasi_http::bindings::http::outgoing_handler::add_to_linker(
            &mut linker,
            |state: &mut Data| state,
        )?;
        wasmtime_wasi_http::bindings::http::types::add_to_linker(
            &mut linker,
            |state: &mut Data| state,
        )?;
        // Don't think we need this, but keeping it for reference
        // Subcommands::add_to_linker(&mut linker, |state: &mut Data| state)?;

        let (instance, _) = Subcommands::instantiate_async(&mut store, &component, &linker).await?;

        let metadata = instance.interface0.call_register(&mut store).await?;
        if let Some(plugin) = self.plugins.insert(
            metadata.id.clone(),
            InstanceData {
                instance,
                metadata: metadata.clone(),
                store,
            },
        ) {
            // Insert the existing plugin back into the map
            let id = plugin.metadata.id.clone();
            self.plugins.insert(plugin.metadata.id.clone(), plugin);
            return Err(anyhow::anyhow!("Plugin with id {id} already exists"));
        }
        Ok(metadata)
    }

    /// Get the metadata for a plugin with the given ID if it exists.
    pub fn metadata(&self, id: &str) -> Option<&Metadata> {
        self.plugins.get(id).map(|p| &p.metadata)
    }

    /// Returns a list of all metadata for all plugins.
    pub fn all_metadata(&self) -> Vec<&Metadata> {
        self.plugins.values().map(|data| &data.metadata).collect()
    }

    /// Run a subcommand with the given name. The plugin will inherit all stdout/stderr/stdin/env
    /// and the remaining non-parsed args and flags. An error will only be returned if there was a
    /// problem with the plugin or the subcommand itself.
    // TODO: We probably want to pass a limited sets of env vars and allowed files here (probably a specific directory space for the plugin to use)
    pub async fn run(&mut self, plugin_id: &str, args: &[impl AsRef<str>]) -> anyhow::Result<()> {
        let plugin = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| anyhow::anyhow!("Plugin with id {plugin_id} does not exist"))?;

        plugin.store.data_mut().ctx = WasiCtxBuilder::new()
            .inherit_env()
            .inherit_network()
            .inherit_stderr()
            .inherit_stdin()
            .inherit_stdio()
            .inherit_stdout()
            .args(args)
            .build();
        plugin
            .instance
            .interface1
            .call_run(&mut plugin.store)
            .await
            .context("Error when running wasm component")?
            .map_err(|_| anyhow::anyhow!("Error when running subcommand"))
    }
}
