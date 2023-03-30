use crate::actor::ModuleConfig;
use crate::capability::{host, logging};
use crate::HandlerBuilder;

use core::fmt;
use core::fmt::Debug;

use anyhow::Context;

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
#[derive(Clone, Default)]
pub struct RuntimeBuilder {
    engine_config: wasmtime::Config,
    handler: HandlerBuilder,
    module_config: ModuleConfig,
}

impl RuntimeBuilder {
    /// Returns a new [`RuntimeBuilder`]
    #[must_use]
    pub fn new() -> Self {
        let mut engine_config = wasmtime::Config::default();
        engine_config.async_support(true);
        engine_config.wasm_component_model(true);
        Self {
            engine_config,
            handler: HandlerBuilder::default(),
            module_config: ModuleConfig::default(),
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

    /// Set a [`host::Host`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn host(self, host: impl host::Host + Sync + Send + 'static) -> Self {
        Self {
            handler: self.handler.host(host),
            ..self
        }
    }

    /// Set a [`logging::Host`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn logging(self, logging: impl logging::Host + Sync + Send + 'static) -> Self {
        Self {
            handler: self.handler.logging(logging),
            ..self
        }
    }

    /// Turns this builder into a [`Runtime`]
    ///
    /// # Errors
    ///
    /// Fails if the configuration is not valid
    pub fn build(self) -> anyhow::Result<Runtime> {
        let engine =
            wasmtime::Engine::new(&self.engine_config).context("failed to construct engine")?;
        Ok(Runtime {
            engine,
            handler: self.handler,
            module_config: self.module_config,
        })
    }
}

impl TryFrom<RuntimeBuilder> for Runtime {
    type Error = anyhow::Error;

    fn try_from(builder: RuntimeBuilder) -> Result<Self, Self::Error> {
        builder.build()
    }
}

/// Shared wasmCloud runtime
#[derive(Clone)]
pub struct Runtime {
    pub(crate) engine: wasmtime::Engine,
    pub(crate) handler: HandlerBuilder,
    pub(crate) module_config: ModuleConfig,
}

impl Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("handler", &self.handler)
            .field("module_config", &self.module_config)
            .field("runtime", &"wasmtime")
            .finish()
    }
}

impl Runtime {
    /// Returns a new [`Runtime`] configured with defaults
    ///
    /// # Errors
    ///
    /// Returns an error if the default configuration is invalid
    pub fn new() -> anyhow::Result<Self> {
        Self::builder().try_into()
    }

    /// Returns a new [`RuntimeBuilder`], which can be used to configure and build a [Runtime]
    #[must_use]
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// [Runtime] version
    #[must_use]
    pub fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
}
