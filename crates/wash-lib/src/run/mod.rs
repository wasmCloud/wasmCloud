use std::collections::HashMap;
use wasmtime::component::ResourceTable;
use wasmtime::{Config, Engine};
use wasmtime_wasi::{WasiCtx, WasiView};

pub use crate::run::workload::Workload;
pub use crate::run::ctx::CtxBuilder;

pub mod ctx;
mod workload;

/// Runs workloads capable of leveraging the local host it runs on.
///
/// Access to the host's resources can be strictly controlled.
/// Follows the principle of least privilege, which means all is denied by
/// default.
pub struct LocalRuntime {
    engine: Engine,
}

impl LocalRuntime {
    pub fn new() -> anyhow::Result<Self> {
        let mut config = Config::new();
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

    pub fn run(&self, workload: &mut Workload) {
        todo!()
    }
}
