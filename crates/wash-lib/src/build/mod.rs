//! Build (and sign) a wasmCloud actor, or provider. Depends on the "cli" feature

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;
use wit_parser::{Resolve, WorldId};

use crate::parser::{ProjectConfig, TypeConfig};

mod component;
pub use component::*;
mod provider;
use provider::*;

/// This tag indicates that a Wasm module uses experimental features of wasmCloud
/// and/or the surrounding ecosystem.
///
/// This tag is normally embedded in a Wasm module as a custom section
const WASMCLOUD_WASM_TAG_EXPERIMENTAL: &str = "wasmcloud.com/experimental";

/// Configuration for signing an artifact (actor or provider) including issuer and subject key, the path to where keys can be found, and an option to
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

/// Using a [ProjectConfig], usually parsed from a `wasmcloud.toml` file, build the project
/// with the installed language toolchain. This will delegate to [build_actor] when the project is an actor,
/// or [build_provider] when the project is a provider.
///
/// This function returns the path to the compiled artifact, a signed Wasm component or signed provider archive.
///
/// # Usage
/// ```no_run
/// use wash_lib::{build::build_project, parser::get_config};
/// let config = get_config(None, Some(true))?;
/// let artifact_path = build_project(config)?;
/// println!("Here is the signed artifact: {}", artifact_path.to_string_lossy());
/// ```
/// # Arguments
/// * `config`: [ProjectConfig] for required information to find, build, and sign an actor
/// * `signing`: Optional [SignConfig] with information for signing the project artifact. If omitted, the artifact will only be built
pub async fn build_project(
    config: &ProjectConfig,
    signing: Option<&SignConfig>,
) -> Result<PathBuf> {
    match &config.project_type {
        TypeConfig::Actor(actor_config) => {
            build_actor(actor_config, &config.language, &config.common, signing)
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
