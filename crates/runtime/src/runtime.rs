use crate::actor::ModuleConfig;
use crate::capability::{
    builtin, Blobstore, Bus, IncomingHttp, KeyValueAtomic, KeyValueEventual, Logging, Messaging,
    OutgoingHttp,
};
use crate::ActorConfig;

use core::fmt;
use core::fmt::Debug;

use std::sync::Arc;

use anyhow::Context;

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
#[derive(Clone, Default)]
pub struct RuntimeBuilder {
    engine_config: wasmtime::Config,
    handler: builtin::HandlerBuilder,
    actor_config: ActorConfig,
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
            handler: builtin::HandlerBuilder::default(),
            actor_config: ActorConfig::default(),
            module_config: ModuleConfig::default(),
        }
    }

    /// Set a custom [`ActorConfig`] to use for all actor instances
    #[must_use]
    pub fn actor_config(self, actor_config: ActorConfig) -> Self {
        Self {
            actor_config,
            ..self
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

    /// Set a [`Blobstore`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn blobstore(self, blobstore: Arc<impl Blobstore + Sync + Send + 'static>) -> Self {
        Self {
            handler: self.handler.blobstore(blobstore),
            ..self
        }
    }

    /// Set a [`Bus`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn bus(self, bus: Arc<impl Bus + Sync + Send + 'static>) -> Self {
        Self {
            handler: self.handler.bus(bus),
            ..self
        }
    }

    /// Set a [`IncomingHttp`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn incoming_http(
        self,
        incoming_http: Arc<impl IncomingHttp + Sync + Send + 'static>,
    ) -> Self {
        Self {
            handler: self.handler.incoming_http(incoming_http),
            ..self
        }
    }

    /// Set a [`KeyValueAtomic`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn keyvalue_atomic(
        self,
        keyvalue_atomic: Arc<impl KeyValueAtomic + Sync + Send + 'static>,
    ) -> Self {
        Self {
            handler: self.handler.keyvalue_atomic(keyvalue_atomic),
            ..self
        }
    }

    /// Set a [`KeyValueEventual`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn keyvalue_eventual(
        self,
        keyvalue_eventual: Arc<impl KeyValueEventual + Sync + Send + 'static>,
    ) -> Self {
        Self {
            handler: self.handler.keyvalue_eventual(keyvalue_eventual),
            ..self
        }
    }

    /// Set a [`Logging`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn logging(self, logging: Arc<impl Logging + Sync + Send + 'static>) -> Self {
        Self {
            handler: self.handler.logging(logging),
            ..self
        }
    }

    /// Set a [`Messaging`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn messaging(self, messaging: Arc<impl Messaging + Sync + Send + 'static>) -> Self {
        Self {
            handler: self.handler.messaging(messaging),
            ..self
        }
    }

    /// Set a [`OutgoingHttp`] handler to use for all actor instances unless overriden for the instance
    #[must_use]
    pub fn outgoing_http(
        self,
        outgoing_http: Arc<impl OutgoingHttp + Sync + Send + 'static>,
    ) -> Self {
        Self {
            handler: self.handler.outgoing_http(outgoing_http),
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
            actor_config: self.actor_config,
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
    pub(crate) handler: builtin::HandlerBuilder,
    pub(crate) actor_config: ActorConfig,
    pub(crate) module_config: ModuleConfig,
}

impl Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("handler", &self.handler)
            .field("actor_config", &self.actor_config)
            .field("module_config", &self.module_config)
            .field("runtime", &"wasmtime")
            .finish_non_exhaustive()
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
