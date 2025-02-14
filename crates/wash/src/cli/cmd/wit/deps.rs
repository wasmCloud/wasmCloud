use std::path::PathBuf;

use crate::lib::build::load_lock_file;
use crate::lib::cli::{CommandOutput, CommonPackageArgs};
use clap::Args;

use crate::lib::parser::{load_config, CommonConfig, ProjectConfig, RegistryConfig};
use wasm_pkg_core::wit::OutputType;

/// Arguments to `wash wit deps`
#[derive(Debug, Args, Clone)]
pub struct DepsArgs {
    /// The directory containing the WIT files to fetch dependencies for.
    #[clap(short = 'd', long = "wit-dir", default_value = "wit")]
    pub dir: PathBuf,

    /// The desired output type of the dependencies. Valid options are "wit" or "wasm" (wasm is the
    /// WIT package binary format).
    #[clap(short = 't', long = "type")]
    pub output_type: Option<OutputType>,

    #[clap(flatten)]
    pub common: CommonPackageArgs,

    /// Path to the wasmcloud.toml file or parent folder to use for building
    #[clap(short = 'p', long = "config-path")]
    config_path: Option<PathBuf>,
}

/// Invoke `wash wit deps`
pub async fn invoke(
    DepsArgs {
        dir,
        common,
        config_path,
        ..
    }: DepsArgs,
) -> anyhow::Result<CommandOutput> {
    // Load wasmcloud.toml configuration, if present
    let project_config = match load_config(config_path.clone(), Some(true)).await {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("failed to load project configuration: {e}");
            None
        }
    };

    // Extract/build configuration for wkg
    let wkg_config = match project_config {
        Some(ProjectConfig {
            ref package_config, ..
        }) => package_config.clone(),
        None => wasm_pkg_core::config::Config::load().await?,
    };

    let project_cfg = load_config(config_path, Some(true)).await?;
    let mut lock_file = load_lock_file(&project_cfg.wasmcloud_toml_dir).await?;

    // Start building the wkg client config
    let mut wkg = crate::lib::deps::WkgFetcher::from_common(&common, wkg_config).await?;
    // If a project configuration was provided, apply any pull-related overrides
    // in the new "extended" configuration format
    if let Some(ProjectConfig {
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
    }) = project_config
    {
        wkg.resolve_extended_pull_configs(&pull_cfg, &wasmcloud_toml_dir)
            .await?;
    }

    // NOTE: Monkey patch & fetch logging performs wkg fetching and placement as a side effect
    wkg.monkey_patch_fetch_logging(dir, &mut lock_file).await?;

    // Now write out the lock file since everything else succeeded
    lock_file.write().await?;

    Ok("Dependencies fetched".into())
}
