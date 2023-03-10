use core::fmt;
use core::fmt::Debug;

use std::sync::Arc;

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
#[derive(Debug)]
pub struct RuntimeBuilder<H> {
    handler: Arc<H>,
}

impl<H> RuntimeBuilder<H> {
    /// Returns a new [`RuntimeBuilder`]
    #[must_use]
    pub fn new(handler: impl Into<Arc<H>>) -> Self {
        Self {
            handler: handler.into(),
        }
    }
}

impl<H> From<RuntimeBuilder<H>> for Runtime<H> {
    fn from(RuntimeBuilder { handler }: RuntimeBuilder<H>) -> Self {
        Self {
            engine: wasmtime::Engine::default(),
            handler,
        }
    }
}

/// Shared wasmCloud runtime
pub struct Runtime<H> {
    pub(crate) engine: wasmtime::Engine,
    pub(crate) handler: Arc<H>,
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
