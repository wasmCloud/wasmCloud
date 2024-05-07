use wasmtime_wasi::{WasiCtx, WasiCtxBuilder};
use crate::registry::OciPullOptions;

/// Controls host accesses.
pub struct Ctx {
    pub(crate) oci_pull_options: OciPullOptions,
    pub(crate) wasi_ctx: WasiCtx,
    // TODO(raskyld): Add `wasi:http` support once basic WASI WIT is supported.
}

/// Builder-style structure used to create a [`Ctx`].
pub struct CtxBuilder {
    wasi: WasiCtxBuilder,
    built: bool,
    oci_pull_options: Option<OciPullOptions>,
}

impl CtxBuilder {
    /// Creates a builder allowing to programmatically define access policies.
    ///
    /// Once you defined your access policies, use the [`Self::build`] function to acquire a [`Ctx`].
    ///
    /// The [`Ctx`] can, then be passed to the [`LocalRuntime::run`][runtime-run] function to
    /// run a component while enforcing those access policies.
    ///
    /// [runtime-run]: crate::run::LocalRuntime::run
    pub fn new() -> Self {
        CtxBuilder {
            wasi: WasiCtxBuilder::new(),
            built: false,
            oci_pull_options: None,
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
            wasi_ctx: self.wasi.build(),
            oci_pull_options: self.oci_pull_options.take().unwrap_or_default(),
        }
    }

    /// Returns a [`WasiCtxBuilder`] that can be used to control access to most WASI
    /// native interfaces.
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

    /// Replaces the default [`OciPullOptions`] with a fine-tuned one.
    pub fn set_oci_pull_options(&mut self, opts: OciPullOptions) -> &mut Self {
        self.oci_pull_options = Some(opts);
        self
    }
}
