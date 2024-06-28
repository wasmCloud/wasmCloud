use crate::registry::OciPullOptions;
use std::path::PathBuf;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder};

/// A structure bearing information about:
/// - Where and how to find the binary of the component to run
/// - Which parts of the host the component may access
pub struct Ctx {
    pub(crate) oci_pull_options: OciPullOptions,
    pub(crate) oci_cache_file: Option<PathBuf>,
    pub(crate) wasi_ctx: WasiCtx,
    pub(crate) reference: String,
    // TODO(raskyld): Add `wasi:http` support once basic WASI WIT is supported.
}

/// Builder-style structure used to create a [`Ctx`].
pub struct CtxBuilder {
    wasi: WasiCtxBuilder,
    built: bool,
    oci_pull_options: Option<OciPullOptions>,
    oci_cache_file: Option<PathBuf>,
    reference: Option<String>,
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
            oci_cache_file: None,
            reference: None,
        }
    }

    /// Builds the final [`Ctx`].
    ///
    /// This method can be called only once. Calling it again will panic.
    ///
    /// # Panics
    ///
    /// Panics if called more than one time.
    pub fn build(&mut self) -> anyhow::Result<Ctx> {
        assert!(!self.built);

        self.built = true;

        if self.reference.is_none() {
            return Err(anyhow::anyhow!("reference cannot be empty"));
        }

        anyhow::Ok(Ctx {
            wasi_ctx: self.wasi.build(),
            oci_pull_options: self.oci_pull_options.take().unwrap_or_default(),
            oci_cache_file: self.oci_cache_file.take(),
            reference: self.reference.take().unwrap(),
        })
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
            .filter(|(env_name, _)| env_name.starts_with(prefix.as_ref()))
            .collect();

        self.wasi.envs(&vars);
        self
    }

    /// Replaces the default [`OciPullOptions`] with a fine-tuned one.
    pub fn set_oci_pull_options(&mut self, opts: OciPullOptions) -> &mut Self {
        self.oci_pull_options = Some(opts);
        self
    }

    /// Sets the path to lookup for a cached binary of the component.
    ///
    /// Cache is never looked-up when using a reference to the local filesystem.
    /// For more information, see [`get_oci_artifact`][crate::registry::get_oci_artifact]
    pub fn set_oci_cache_path(&mut self, path: PathBuf) -> &mut Self {
        self.oci_cache_file = Some(path);
        self
    }

    /// Sets the reference of the component to run.
    ///
    /// The reference is resolved using [`get_oci_artifact`][crate::registry::get_oci_artifact]
    pub fn set_reference(&mut self, reference: String) -> &mut Self {
        self.reference = Some(reference);
        self
    }
}

impl Default for CtxBuilder {
    fn default() -> Self {
        Self::new()
    }
}
