//! Build (and sign) a wasmCloud component, or provider. Depends on the "cli" feature

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use semver::VersionReq;
use tracing::info;
use wasm_pkg_client::{
    caching::{CachingClient, FileCache},
    PackageRef,
};
use wasm_pkg_core::{config::Override, lock::LockFile, wit::OutputType};
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
        let client = package_args.get_client().await?;
        let mut lock = load_lock_file(&config.wasmcloud_toml_dir).await?;

        monkey_patch_fetch_logging(
            config.package_config.clone(),
            &config.common.wit_dir,
            &mut lock,
            client,
        )
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

/// This is a hacky, monkey-patch helper for the fact that the wasi:logging package is not versioned
/// in the host, which makes it hard to use with packaging tools. We have added a version, but
/// pretty much everything uses the versionless wasi:logging package. This function wraps the normal
/// dependency fetching steps, checking if the package has a wasi:logging dep that isn't versioned.
/// If it does have the unversioned one, then the hackery commences to do some string replacements
/// in the wit files in a temp dir, pulls down the dependencies, and then removes the versioned wit.
/// This is ugliness in the highest degree, but it is the only way to get the logging package to
/// work with the packaging tools. The current libraries don't really support printing unresolved
/// packages or substituting things in (which makes sense), so this is what we have to live with
///
/// DO NOT USE THIS unless you know what you are doing. This function is exempted from any semver
/// guarantees and will be removed as soon as we move to the properly versioned wasi:logging
/// package.
#[doc(hidden)]
pub async fn monkey_patch_fetch_logging(
    mut wkg_conf: wasm_pkg_core::config::Config,
    wit_dir: impl AsRef<Path>,
    lock: &mut LockFile,
    client: CachingClient<FileCache>,
) -> Result<()> {
    let wasi_logging_name: PackageRef = "wasi:logging".parse().unwrap();
    // This is inefficient since we have to load this again when we fetch deps, but we need to do
    // this to get the list of packages from the package
    let (_, packages) = wasm_pkg_core::wit::get_packages(&wit_dir)
        .context("failed to get packages from wit dir")?;
    // If there is a depenency on unversioned wasi:logging, add an override (if not present)
    let patch_dir = if packages.contains(&(wasi_logging_name.clone(), VersionReq::STAR)) {
        // copy all top level wit files to a temp dir. All the stuff people should be doing at the top
        // level so this is fine
        let wit_dir_temp = tokio::task::spawn_blocking(tempfile::tempdir)
            .await
            .context("failed to create temporary wit patch directory")?
            .context("failed to create temporary wit patch directory")?;
        let mut readdir = tokio::fs::read_dir(&wit_dir)
            .await
            .context("failed to read temporary wit patch directory")?;
        while let Some(entry) = readdir
            .next_entry()
            .await
            .context("failed to read entry in temporary wit patch directory")?
        {
            let path = entry.path();
            let meta = entry
                .metadata()
                .await
                .context("failed to get metadata for entry in temporary wit patch directory")?;

            if meta.is_file() && path.extension().unwrap_or_default() == "wit" {
                // Read all data as a string and replace
                let data = tokio::fs::read_to_string(&path).await.context(
                    "failed to read interface for entry in temporary wit patch directory",
                )?;
                let data = data.replace("wasi:logging/logging", "wasi:logging/logging@0.1.0-draft");
                tokio::fs::write(wit_dir_temp.path().join(path.file_name().unwrap()), data)
                    .await
                    .context(
                        "failed to write interface for entry in temporary wit patch directory",
                    )?;
            }
        }
        // set the overrides
        let overrides = wkg_conf.overrides.get_or_insert_with(HashMap::new);
        if let std::collections::hash_map::Entry::Vacant(e) =
            overrides.entry(wasi_logging_name.to_string())
        {
            e.insert(Override {
                version: Some("=0.1.0-draft".parse().unwrap()),
                ..Default::default()
            });
        }
        Some(wit_dir_temp)
    } else {
        None
    };

    wasm_pkg_core::wit::fetch_dependencies(
        &wkg_conf,
        patch_dir
            .as_ref()
            .map(|t| t.path())
            .unwrap_or(wit_dir.as_ref()),
        lock,
        client,
        OutputType::Wit,
    )
    .await?;

    if let Some(patch_dir) = patch_dir {
        // Rewrite the logging dep to not have a version
        let dep_path = patch_dir
            .path()
            .join("deps")
            .join("wasi-logging-0.1.0-draft")
            .join("package.wit");
        let contents = tokio::fs::read_to_string(&dep_path).await?;
        let replaced =
            contents.replace("package wasi:logging@0.1.0-draft;", "package wasi:logging;");
        tokio::fs::write(&dep_path, replaced)
            .await
            .context("Unable to write patched logging dependency")?;
        // Remove the destination deps
        let dest_deps_dir = wit_dir.as_ref().join("deps");
        match tokio::fs::remove_dir_all(&dest_deps_dir).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        };
        // Copy the deps dir back
        copy_dir(patch_dir.path().join("deps"), dest_deps_dir).await?;
    }
    Ok(())
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

async fn copy_dir(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(&destination).await?;
    let mut entries = tokio::fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let filetype = entry.file_type().await?;
        if filetype.is_dir() {
            Box::pin(copy_dir(
                entry.path(),
                destination.as_ref().join(entry.file_name()),
            ))
            .await?;
        } else {
            tokio::fs::copy(entry.path(), destination.as_ref().join(entry.file_name())).await?;
        }
    }
    Ok(())
}
