use core::fmt;
use core::fmt::Debug;

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
#[derive(Debug, Default)]
pub struct RuntimeBuilder;

impl RuntimeBuilder {
    /// Returns a new [`RuntimeBuilder`]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl From<RuntimeBuilder> for Runtime {
    fn from(RuntimeBuilder {}: RuntimeBuilder) -> Self {
        Self {
            engine: wasmtime::Engine::default(),
        }
    }
}

/// Shared wasmCloud runtime
pub struct Runtime {
    pub(crate) engine: wasmtime::Engine,
}

impl Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("runtime", &"wasmtime")
            .finish()
    }
}

impl Runtime {
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
