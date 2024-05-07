use std::collections::HashMap;
use wasmtime::{Config, Engine};
use wasmtime_wasi::{WasiCtx, WasiView};
use anyhow::{Context, Result};
use wasmtime::component::Component;
use crate::cli::cached_oci_file;
use crate::registry::{get_oci_artifact, OciPullOptions};
use crate::run::ctx::Ctx;

pub use crate::run::ctx::CtxBuilder;

pub mod ctx;

/// Runs workloads capable of leveraging the local host it runs on.
///
/// Access to the host's resources can be strictly controlled.
/// Follows the principle of least privilege, which means all is denied by
/// default.
pub struct LocalRuntime {
    engine: Engine,
}

impl LocalRuntime {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();

        // Try to read settings from default location
        if let Err(e) = config.cache_config_load_default() {
            tracing::warn!(err = ?e, "Failed to load wasm cache");
        }

        config.wasm_component_model(true);
        config.async_support(true);

        let engine = Engine::new(&config)?;
        anyhow::Ok(Self {
            engine,
        })
    }

    pub async fn run(&self, reference: String, context: Ctx) -> Result<()> {
        let artifact = get_oci_artifact(reference.clone(), Some(cached_oci_file(&reference)), context.oci_pull_options.clone())
            .context("failed to pull the component")
            .await?;

        let component = Component::new(&self.engine, &artifact)
            .context("failed to build the pulled component")?;

        //NB(raskyld): Not sure if the `data` stored needs to be owned by the Store itself.
        let mut store = wasmtime::Store::new(&self.engine, State::new(context));
        let mut linker = wasmtime::Linker::new(&self.engine);

        wasmtime_wasi::bindings::Command::add_to_linker(
            &mut linker,
            |state: &mut State| state,
        )?;

        //TODO(raskyld): add http host funcs

        let (instance, _) = wasmtime_wasi::bindings::Command::instantiate_async(&mut store, &component, &linker)
            .await
            .context("failed to instantiate the compiled component")?;

        instance
            .wasi_cli_run()
            .call_run(&mut store)
            .await
            .context("failed to run the component")?
            .map_err(|_| anyhow::anyhow!("the component has been run but returned an error"))
    }
}

struct State {
    resources: wasmtime::component::ResourceTable,
    context: Ctx,
}

impl State {
    fn new(ctx: Ctx) -> Self {
        State {
            resources: wasmtime::component::ResourceTable::new(),
            context: ctx,
        }
    }
}

impl WasiView for State {
    fn table(&mut self) -> &mut wasmtime::component::ResourceTable {
        &mut self.resources
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.context.wasi_ctx
    }
}
