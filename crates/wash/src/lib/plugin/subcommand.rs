mod bindings {
    wasmtime::component::bindgen!({
        world: "subcommands",
        async: true,
    });
}

pub use bindings::exports::wasmcloud::wash::subcommand::Metadata;
use bindings::Subcommands;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Ok};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};
use wasmtime_wasi_http::WasiHttpCtx;

use super::Data;

const DIRECTORY_ALLOW: DirPerms = DirPerms::all();
const DIRECTORY_DENY: DirPerms = DirPerms::READ;

struct InstanceData {
    instance: Subcommands,
    metadata: Metadata,
    loaded_path: PathBuf,
    store: wasmtime::Store<Data>,
}

/// A struct that manages loading and running subcommand plugins
pub struct SubcommandRunner {
    engine: Engine,
    plugins: HashMap<String, InstanceData>,
}

/// Host directory mapping to provide to plugins
pub struct DirMapping {
    /// The path on the host that should be opened. If this is a file, its parent directory will be
    /// added with no RW access, but with RW access to the files in that directory. If it is a
    /// directory, it will be added with RW access to that directory
    pub host_path: PathBuf,
    /// The path that will be accessible in the component. Otherwise defaults to the `host_path`
    pub component_path: Option<String>,
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
            http: WasiHttpCtx::new(),
        };

        let mut store = wasmtime::Store::new(&self.engine, ctx);

        let component = Component::from_file(&self.engine, &path)?;
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::add_to_linker_async(&mut linker)
            .context("failed to link core WASI interfaces")?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)
            .context("failed to link `wasi:http`")?;

        let instance = Subcommands::instantiate_async(&mut store, &component, &linker).await?;
        let metadata = instance
            .wasmcloud_wash_subcommand()
            .call_register(&mut store)
            .await?;
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
    #[must_use]
    pub fn metadata(&self, id: &str) -> Option<&Metadata> {
        self.plugins.get(id).map(|p| &p.metadata)
    }

    /// Returns a list of all metadata for all plugins.
    #[must_use]
    pub fn all_metadata(&self) -> Vec<&Metadata> {
        self.plugins.values().map(|data| &data.metadata).collect()
    }

    /// Returns the path to the plugin with the given ID.
    #[must_use]
    pub fn path(&self, id: &str) -> Option<&Path> {
        self.plugins.get(id).map(|p| p.loaded_path.as_path())
    }

    /// Run a subcommand with the given name and args. The plugin will inherit all
    /// stdout/stderr/stdin/env. The given `plugin_dirs` will be mapped into the plugin after
    /// canonicalizing all paths and normalizing them to use `/` instead of `\`. An error will only
    /// be returned if there was a problem with the plugin (such as the plugin dirs not existing or
    /// failure to canonicalize) or the subcommand itself.
    ///
    /// All plugins will be passed environment variables starting with
    /// `WASH_PLUGIN_${plugin_id.to_upper()}_` from the current process. Other vars will be ignored
    pub async fn run(
        &mut self,
        plugin_id: &str,
        plugin_dir: PathBuf,
        dirs: Vec<DirMapping>,
        mut args: Vec<String>,
    ) -> anyhow::Result<()> {
        let plugin = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| anyhow::anyhow!("Plugin with id {plugin_id} does not exist"))?;

        let env_prefix = format!("WASH_PLUGIN_{}_", plugin_id.to_uppercase());
        let vars: Vec<_> = std::env::vars()
            .filter(|(k, _)| k.starts_with(&env_prefix))
            .collect();
        let mut ctx = WasiCtxBuilder::new();
        for dir in dirs {
            // To avoid relative dirs and permissions issues, we canonicalize the host path
            let canonicalized = tokio::fs::canonicalize(&dir.host_path)
                .await
                .context("Error when canonicalizing given path")?;
            // We need this later and will have to return an error anyway if this fails
            let str_canonical = canonicalized.to_str().ok_or_else(|| anyhow::anyhow!("Canonicalized path cannot be converted to a string for use in a plugin. This is a limitation of the WASI API"))?.to_string();
            // Check if the path is a file or a dir so we can handle permissions accordingly
            let is_dir = tokio::fs::metadata(&canonicalized)
                .await
                .map(|m| m.is_dir())
                .context("Error when checking if path is a file or a dir")?;
            let (host_path, guest_path, dir_perms) = match (is_dir, dir.component_path) {
                (true, Some(path)) => (canonicalized.clone(), path, DIRECTORY_ALLOW),
                (false, Some(path)) => (
                    canonicalized
                        .parent()
                        .ok_or_else(|| anyhow::anyhow!("Could not get parent of given file"))?
                        .to_path_buf(),
                    path,
                    DIRECTORY_DENY,
                ),
                (true, None) => (
                    canonicalized.clone(),
                    str_canonical.clone(),
                    DIRECTORY_ALLOW,
                ),
                (false, None) => {
                    let parent = canonicalized
                        .parent()
                        .ok_or_else(|| anyhow::anyhow!("Could not get parent of given file"))?
                        .to_path_buf();
                    (
                        parent.clone(),
                        // SAFETY: We already checked that canonicalized was a string above so we
                        // can just unwrap here
                        parent.to_str().unwrap().to_string(),
                        DIRECTORY_DENY,
                    )
                }
            };

            // On Windows, we need to normalize the path separators to "/" since that is what is
            // expected by things like `PathBuf` when built for WASI.
            #[cfg(target_family = "windows")]
            let guest_path = guest_path.replace('\\', "/");
            #[cfg(target_family = "windows")]
            let str_canonical = str_canonical.replace('\\', "/");
            ctx.preopened_dir(host_path, guest_path, dir_perms, FilePerms::all())
                .context("Error when preopening path argument")?;
            // Substitute the path in the args with the canonicalized path
            let matching = args
                .iter_mut()
                .find(|arg| {
                    <&mut std::string::String as std::convert::AsRef<Path>>::as_ref(arg)
                        == dir.host_path
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Could not find host path {} in args for replacement",
                        dir.host_path.display()
                    )
                })?;
            *matching = str_canonical;
        }
        // Disable socket connections for now. We may gradually open this up later
        ctx.socket_addr_check(|_, _| Box::pin(async { false }))
            .inherit_stdio()
            .preopened_dir(plugin_dir, "/", DIRECTORY_ALLOW, FilePerms::all())
            .context("Error when preopening plugin dir")?
            .args(&args)
            .envs(&vars);

        plugin.store.data_mut().ctx = ctx.build();
        plugin
            .instance
            .wasi_cli_run()
            .call_run(&mut plugin.store)
            .await
            .context("Error when running wasm component")?
            .map_err(|()| anyhow::anyhow!("Error when running subcommand"))
    }
}
