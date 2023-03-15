use crate::actor::ModuleConfig;
use crate::capability::{Handle, HandlerBuilder, HostInvocation, Invocation};

use core::fmt;
use core::fmt::Debug;

use std::sync::Arc;

use anyhow::Context;

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
pub struct RuntimeBuilder {
    handler: Arc<Box<dyn Handle<Invocation>>>,
    module_config: ModuleConfig,
    engine_config: wasmtime::Config,
}

impl RuntimeBuilder {
    /// Returns a new [`RuntimeBuilder`]
    #[must_use]
    pub fn new(handler: impl Into<Arc<Box<dyn Handle<Invocation>>>>) -> Self {
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
pub struct Runtime {
    pub(crate) engine: wasmtime::Engine,
    pub(crate) handler: Arc<Box<dyn Handle<Invocation>>>,
    pub(crate) module_config: ModuleConfig,
}

impl Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("runtime", &"wasmtime")
            .field("module_config", &self.module_config)
            .finish()
    }
}

impl Runtime {
    /// Returns a new [`RuntimeBuilder`], which can be used to configure and build a [Runtime]
    #[must_use]
    pub fn builder(handler: impl Into<Arc<Box<dyn Handle<Invocation>>>>) -> RuntimeBuilder {
        RuntimeBuilder::new(handler)
    }

    /// Constructs a new [Runtime] given a handler, which handles both builtins and host calls
    ///
    /// # Errors
    ///
    /// Fails if [`Self::new`] with the resulting handler fails
    pub fn new(handler: impl Into<Arc<Box<dyn Handle<Invocation>>>>) -> anyhow::Result<Self> {
        Self::builder(handler).build()
    }

    /// Constructs a new [Runtime] given a host call handler and using default for everything else
    ///
    /// # Errors
    ///
    /// Fails if [`Self::new`] with the resulting handler fails
    pub fn from_host_handler(
        host: impl Handle<HostInvocation> + 'static,
    ) -> anyhow::Result<Runtime> {
        Self::new(HandlerBuilder::new(host))
    }

    /// [Runtime] version
    #[must_use]
    pub fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
}
