use wasmtime_wasi::{WasiCtx, WasiCtxBuilder};

/// Controls host accesses.
pub struct Ctx {
    pub(crate) wasi_ctx: WasiCtx,
    // TODO(raskyld): Add `wasi:http` support once basic WASI WIT is supported.
}

/// Builder-style structure used to create a [`Ctx`].
///
/// At the moment, it's a simple wrapper around [`WasiCtxBuilder`] but we
/// plan to support more access control mechanisms in the future.
pub struct CtxBuilder {
    wasi: WasiCtxBuilder,
    built: bool,
}

impl CtxBuilder {
    pub fn new() -> Self {
        CtxBuilder {
            wasi: WasiCtxBuilder::new(),
            built: false,
        }
    }

    /// Builds the final [`Ctx`].
    ///
    /// This method can be called only once. Calling it again will panic.
    ///
    /// # Panics
    ///
    /// Panics if called more than one time.
    pub fn build(&mut self) -> Ctx {
        assert!(!self.built);

        self.built = true;

        Ctx {
            wasi_ctx: self.wasi.build()
        }
    }

    pub fn wasi_ctx(&mut self) -> &mut WasiCtxBuilder {
        &mut self.wasi
    }

    /// Allows accessing any environment variable whose name starts with
    /// `prefix`.
    pub fn env_with_prefix(&mut self, prefix: impl AsRef<str>) -> &mut Self {
        let vars: Vec<_> = std::env::vars()
            .filter(|(env_name, _)| env_name.starts_with(&prefix))
            .collect();

        self.wasi.envs(&vars);
        self
    }
}
