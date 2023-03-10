use core::fmt;
use core::fmt::Debug;

use std::sync::Arc;

/// `WebAssembly` module instance configuration
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct InstanceConfig {
    /// Minimum amount of WebAssembly memory pages to allocate for WebAssembly module instance.
    ///
    /// A WebAssembly memory page size is 64k.
    pub min_memory_pages: u32,
    /// WebAssembly memory page allocation limit for a WebAssembly module instance.
    ///
    /// A WebAssembly memory page size is 64k.
    pub max_memory_pages: Option<u32>,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            min_memory_pages: 4,
            max_memory_pages: None,
        }
    }
}

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
#[derive(Debug)]
pub struct RuntimeBuilder<H> {
    handler: Arc<H>,
    instance_config: InstanceConfig,
}

impl<H> RuntimeBuilder<H> {
    /// Returns a new [`RuntimeBuilder`]
    #[must_use]
    pub fn new(handler: impl Into<Arc<H>>) -> Self {
        Self {
            handler: handler.into(),
            instance_config: InstanceConfig::default(),
        }
    }

    /// Set a custom [`InstanceConfig`] to use for all `WebAssembly` module instances
    #[must_use]
    pub fn instance_config(self, instance_config: InstanceConfig) -> Self {
        Self {
            instance_config,
            ..self
        }
    }
}

impl<H> From<RuntimeBuilder<H>> for Runtime<H> {
    fn from(
        RuntimeBuilder {
            handler,
            instance_config,
        }: RuntimeBuilder<H>,
    ) -> Self {
        Self {
            engine: wasmtime::Engine::default(),
            handler,
            instance_config,
        }
    }
}

/// Shared wasmCloud runtime
pub struct Runtime<H> {
    pub(crate) engine: wasmtime::Engine,
    pub(crate) handler: Arc<H>,
    pub(crate) instance_config: InstanceConfig,
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
