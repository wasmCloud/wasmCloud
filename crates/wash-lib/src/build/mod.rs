//! Build (and sign) a wasmCloud component, or provider. Depends on the "cli" feature

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;
use wasm_pkg_core::{lock::LockFile, wit::OutputType};
use wit_parser::{Resolve, WorldId};

use crate::{
    cli::CommonPackageArgs,
    parser::{ProjectConfig, TypeConfig},
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
/// ```no_run
/// use wash_lib::{build::build_project, parser::get_config};
/// let config = get_config(None, Some(true))?;
/// let artifact_path = build_project(&config, None)?;
/// println!("Here is the signed artifact: {}", artifact_path.to_string_lossy());
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
    if !skip_fetch {
        // Fetch dependencies for the component before building
        let client = package_args.get_client().await?;
        let lock_path = &config.common.path.join(wasm_pkg_core::lock::LOCK_FILE_NAME);
        let mut lock = if tokio::fs::try_exists(&lock_path).await? {
            LockFile::load_from_path(lock_path, false).await?
        } else {
            let mut lock = LockFile::new_with_path([], lock_path).await?;
            // If it is a new file, write the empty file now in case the next step fails
            lock.write().await?;
            lock
        };
        let conf_path = &config
            .common
            .path
            .join(wasm_pkg_core::config::CONFIG_FILE_NAME);
        let wkg_conf = if tokio::fs::try_exists(&conf_path).await? {
            wasm_pkg_core::config::Config::load_from_path(conf_path).await?
        } else {
            wasm_pkg_core::config::Config::default()
        };

        wasm_pkg_core::wit::fetch_dependencies(
            &wkg_conf,
            &config.common.path.join("wit"),
            &mut lock,
            client,
            OutputType::Wit,
        )
        .await?;
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
