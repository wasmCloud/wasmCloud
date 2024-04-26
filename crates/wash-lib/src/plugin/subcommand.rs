wasmtime::component::bindgen!({
    world: "subcommands",
    async: true,
});

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Ok};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};
use wasmtime_wasi_http::WasiHttpCtx;

use super::Data;
use exports::wasmcloud::wash::subcommand::Metadata;

struct InstanceData {
    instance: Subcommands,
    metadata: Metadata,
    loaded_path: PathBuf,
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
        self.add_plugin_internal(path, false).await
    }

    /// Same as [`add_plugin`](Self::add_plugin), but will not return an error if the plugin exists,
    /// instead updating the metadata for the plugin. This is an upsert operation and will register
    /// the plugin if it does not exist.
    pub async fn update_plugin(&mut self, path: impl AsRef<Path>) -> anyhow::Result<Metadata> {
        self.add_plugin_internal(path, true).await
    }

    async fn add_plugin_internal(
        &mut self,
        path: impl AsRef<Path>,
        update: bool,
    ) -> anyhow::Result<Metadata> {
        // We create a bare context here for registration and then update the store with a new context before running
        let ctx = WasiCtxBuilder::new().build();

        let ctx = Data {
            table: wasmtime::component::ResourceTable::default(),
            ctx,
            http: WasiHttpCtx,
        };

        let mut store = wasmtime::Store::new(&self.engine, ctx);

        let component = Component::from_file(&self.engine, &path)?;
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

        let (instance, _) = Subcommands::instantiate_async(&mut store, &component, &linker).await?;

        let metadata = instance.interface0.call_register(&mut store).await?;
        let maybe_existing = self.plugins.insert(
            metadata.id.clone(),
            InstanceData {
                instance,
                metadata: metadata.clone(),
                loaded_path: path.as_ref().to_owned(),
                store,
            },
        );

        match (update, maybe_existing) {
            // If we're updating and the plugin exists already, overwrite is ok.
            (true, _) | (false, None) => Ok(metadata),
            // If update isn't set, then we don't allow the update
            (false, Some(plugin)) => {
                // Insert the existing plugin back into the map
                let id = plugin.metadata.id.clone();
                self.plugins.insert(plugin.metadata.id.clone(), plugin);
                Err(anyhow::anyhow!("Plugin with id {id} already exists"))
            }
        }
    }

    /// Get the metadata for a plugin with the given ID if it exists.
    pub fn metadata(&self, id: &str) -> Option<&Metadata> {
        self.plugins.get(id).map(|p| &p.metadata)
    }

    /// Returns a list of all metadata for all plugins.
    pub fn all_metadata(&self) -> Vec<&Metadata> {
        self.plugins.values().map(|data| &data.metadata).collect()
    }

    /// Returns the path to the plugin with the given ID.
    pub fn path(&self, id: &str) -> Option<&Path> {
        self.plugins.get(id).map(|p| p.loaded_path.as_path())
    }

    /// Run a subcommand with the given name and args. The plugin will inherit all
    /// stdout/stderr/stdin/env. The given plugin_dir is used to grant the plugin access to the
    /// filesystem in a specific directory, and should already exist. An error will only be returned
    /// if there was a problem with the plugin (such as the plugin_dir not existing) or the
    /// subcommand itself.
    ///
    /// All plugins will be passed environment variables starting with
    /// `WASH_PLUGIN_${plugin_id.to_upper()}_` from the current process. Other vars will be ignored
    pub async fn run(
        &mut self,
        plugin_id: &str,
        plugin_dir: impl AsRef<Path>,
        args: &[impl AsRef<str>],
    ) -> anyhow::Result<()> {
        let plugin = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| anyhow::anyhow!("Plugin with id {plugin_id} does not exist"))?;

        let dir = cap_std::fs::Dir::open_ambient_dir(plugin_dir, cap_std::ambient_authority())
            .context("Failed to open plugin directory")?;
        let env_prefix = format!("WASH_PLUGIN_{}_", plugin_id.to_uppercase());
        let vars: Vec<_> = std::env::vars()
            .filter(|(k, _)| k.starts_with(&env_prefix))
            .collect();
        plugin.store.data_mut().ctx = WasiCtxBuilder::new()
            .inherit_network()
            .inherit_stderr()
            .inherit_stdin()
            .inherit_stdio()
            .inherit_stdout()
            .preopened_dir(dir, DirPerms::all(), FilePerms::all(), "/")
            .args(args)
            .envs(&vars)
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
