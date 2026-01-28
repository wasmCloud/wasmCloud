//! Build (and sign) a wasmCloud component, or provider. Depends on the "cli" feature

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;
use wasm_pkg_core::lock::LockFile;
use wit_parser::{Resolve, WorldId};

use crate::lib::{
    cli::CommonPackageArgs,
    deps::WkgFetcher,
    parser::{CommonConfig, ProjectConfig, RegistryConfig, TypeConfig},
};

mod component;
pub use component::*;
mod provider;
use provider::build_provider;

/// This tag indicates that a Wasm module uses experimental features of wasmCloud
/// and/or the surrounding ecosystem.
///
/// This tag is normally embedded in a Wasm module as a custom section
const WASMCLOUD_WASM_TAG_EXPERIMENTAL: &str = "wasmcloud.com/experimental";
const WIT_DEPS_TOML: &str = "deps.toml";

/// The default name of the package locking file for wasmcloud
pub const PACKAGE_LOCK_FILE_NAME: &str = "wasmcloud.lock";

/// A helper function for loading a lockfile in a given directory, using the existing wkg.lock if it
/// exists. Returns an exclusively locked lockfile.
pub async fn load_lock_file(dir: impl AsRef<Path>) -> Result<LockFile> {
    // First check if a wkg.lock exists in the directory. If it does, load it instead
    let maybe_wkg_path = dir.as_ref().join(wasm_pkg_core::lock::LOCK_FILE_NAME);
    if tokio::fs::try_exists(&maybe_wkg_path).await? {
        return LockFile::load_from_path(&maybe_wkg_path, false)
            .await
            .context("failed to load lock file");
    }
    // Now try to load the wasmcloud one. If it exists, load, otherwise return an empty lock file
    let lock_file_path = dir.as_ref().join(PACKAGE_LOCK_FILE_NAME);
    if tokio::fs::try_exists(&lock_file_path)
        .await
        .context("failed to check if lock file exists")?
    {
        LockFile::load_from_path(lock_file_path, false)
            .await
            .context("failed to load lock file")
    } else {
        let mut lock_file = LockFile::new_with_path([], lock_file_path)
            .await
            .context("failed to create lock file")?;
        lock_file
            .write()
            .await
            .context("failed to write newly created lock file")?;
        Ok(lock_file)
    }
}

/// Configuration for signing an artifact (component or provider) including issuer and subject key, the path to where keys can be found, and an option to
/// disable automatic key generation if keys cannot be found.
#[derive(Debug, Clone, Default)]
pub struct SignConfig {
    /// Location of key files for signing
    pub keys_directory: Option<PathBuf>,

    /// Path to issuer seed key (account). If this flag is not provided, the seed will be sourced from ($HOME/.wash/keys) or generated for you if it cannot be found.
    pub issuer: Option<String>,

    /// Path to subject seed key (module or service). If this flag is not provided, the seed will be sourced from ($HOME/.wash/keys) or generated for you if it cannot be found.
    pub subject: Option<String>,

    /// Disables autogeneration of keys if seed(s) are not provided
    pub disable_keygen: bool,
}

/// Using a [`ProjectConfig`], usually parsed from a `wasmcloud.toml` file, build the project
/// with the installed language toolchain. This will delegate to [`build_component`] when the project is an component,
/// or [`build_provider`] when the project is a provider.
///
/// This function returns the path to the compiled artifact, a signed Wasm component or signed provider archive.
///
/// # Usage
/// ```
/// # async fn doc(
/// #     config: &wash_lib::parser::ProjectConfig,
/// #     package_args: &wash_lib::cli::CommonPackageArgs,
/// #     skip_fetch: bool,
/// # ) -> anyhow::Result<()> {
/// # use wash_lib::build::build_project;
///   let artifact_path = build_project(config, None, package_args, skip_fetch).await?;
///   println!(
///       "Here is the signed artifact: {}",
///       artifact_path.to_string_lossy()
///   );
/// # anyhow::Ok(())
/// }
/// ```
/// # Arguments
/// * `config`: [`ProjectConfig`] for required information to find, build, and sign a component
/// * `signing`: Optional [`SignConfig`] with information for signing the project artifact. If omitted, the artifact will only be built
pub async fn build_project(
    config: &ProjectConfig,
    signing: Option<&SignConfig>,
    package_args: &CommonPackageArgs,
    skip_fetch: bool,
) -> Result<PathBuf> {
    // NOTE(lxf): Check if deps.toml is in config.common.wit_dir, if it is, we skip fetching.
    // This means the project hasn't been converted to wkg yet.
    let wit_deps_exists = tokio::fs::try_exists(config.common.wit_dir.join(WIT_DEPS_TOML)).await?;

    if wit_deps_exists {
        info!("Skipping fetching dependencies because deps.toml exists in the wit directory. Use 'wit-deps' to fetch dependencies.");
    }

    let wit_dir_exists = tokio::fs::metadata(&config.common.wit_dir).await.is_ok();
    if !wit_dir_exists {
        info!("Skipping fetching dependencies because the wit directory does not exist.");
        info!("Assuming that dependencies are included in the project.");
    }

    if !skip_fetch && !wit_deps_exists && wit_dir_exists {
        // Fetch dependencies for the component before building
        let mut wkg = WkgFetcher::from_common(package_args, config.package_config.clone()).await?;
        // If a project configuration was provided, apply any pull-related overrides
        // in the new "extended" configuration format
        if let ProjectConfig {
            common:
                CommonConfig {
                    registry:
                        RegistryConfig {
                            pull: Some(pull_cfg),
                            ..
                        },
                    ..
                },
            wasmcloud_toml_dir,
            ..
        } = config
        {
            wkg.resolve_extended_pull_configs(pull_cfg, &wasmcloud_toml_dir)
                .await?;
        }

        let mut lock = load_lock_file(&config.wasmcloud_toml_dir).await?;

        wkg.monkey_patch_fetch_logging(&config.common.wit_dir, &mut lock)
            .await
            .context("Failed to update dependencies")?;

        // Write out the lock file
        lock.write()
            .await
            .context("Unable to write lock file for dependencies")?;
    }

    match &config.project_type {
        TypeConfig::Component(component_config) => {
            build_component(component_config, &config.language, &config.common, signing).await
        }
        TypeConfig::Provider(provider_config) => {
            build_provider(provider_config, &config.language, &config.common, signing).await
        }
    }
}

/// Build a [`wit_parser::Resolve`] from a provided directory
/// and select a given world
fn convert_wit_dir_to_world(
    dir: impl AsRef<Path>,
    world: impl AsRef<str>,
) -> Result<(Resolve, WorldId)> {
    // Resolve the WIT directory packages & worlds
    let mut resolve = wit_parser::Resolve::default();
    let (package_id, _paths) = resolve
        .push_dir(dir.as_ref())
        .with_context(|| format!("failed to add WIT directory @ [{}]", dir.as_ref().display()))?;
    info!("successfully loaded WIT @ [{}]", dir.as_ref().display());

    // Select the target world that was specified by the user
    let world_id = resolve
        .select_world(package_id, world.as_ref().into())
        .context("failed to select world from built resolver")?;

    Ok((resolve, world_id))
}
