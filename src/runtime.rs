use crate::actor::ModuleConfig;

use core::fmt;
use core::fmt::Debug;

use std::sync::Arc;

use anyhow::Context;

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
#[derive(Debug)]
pub struct RuntimeBuilder<H> {
    handler: Arc<H>,
    module_config: ModuleConfig,
    engine_config: wasmtime::Config,
}

impl<H> RuntimeBuilder<H> {
    /// Returns a new [`RuntimeBuilder`]
    #[must_use]
    pub fn new(handler: impl Into<Arc<H>>) -> Self {
        let mut engine_config = wasmtime::Config::default();
        engine_config.async_support(true);
        #[cfg(feature = "component-model")]
        engine_config.wasm_component_model(true);
        Self {
            handler: handler.into(),
            module_config: ModuleConfig::default(),
            engine_config,
        }
    }

    /// Set a custom [`ModuleConfig`] to use for all actor module instances
    #[must_use]
    pub fn module_config(self, module_config: ModuleConfig) -> Self {
        Self {
            module_config,
            ..self
        }
    }
}

impl<H> TryFrom<RuntimeBuilder<H>> for Runtime<H> {
    type Error = anyhow::Error;

    fn try_from(
        RuntimeBuilder {
            engine_config,
            handler,
            module_config,
        }: RuntimeBuilder<H>,
    ) -> Result<Self, Self::Error> {
        let engine = wasmtime::Engine::new(&engine_config).context("failed to construct engine")?;
        Ok(Self {
            engine,
            handler,
            module_config,
        })
    }
}

/// Shared wasmCloud runtime
pub struct Runtime<H> {
    pub(crate) engine: wasmtime::Engine,
    pub(crate) handler: Arc<H>,
    pub(crate) module_config: ModuleConfig,
}

impl<H> Debug for Runtime<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("runtime", &"wasmtime")
            .finish()
    }
}

impl<H> Runtime<H> {
    /// Returns a new [`RuntimeBuilder`], which can be used to configure and build a [Runtime]
    #[must_use]
    pub fn builder(handler: impl Into<Arc<H>>) -> RuntimeBuilder<H> {
        RuntimeBuilder::new(handler.into())
    }

    /// [Runtime] version
    #[must_use]
    pub fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
}
