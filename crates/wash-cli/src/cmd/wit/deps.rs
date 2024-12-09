use std::path::PathBuf;

use anyhow::{bail, Context};
use clap::Args;
use wash_lib::build::{load_lock_file, monkey_patch_fetch_logging};
use wash_lib::cli::{CommandOutput, CommonPackageArgs};
use wash_lib::parser::{
    load_config, CommonConfig, ProjectConfig, RegistryConfig, RegistryPullSource,
    RegistryPullSourceOverride,
};
use wasm_pkg_client::PackageRef;
use wasm_pkg_core::config::Override;
use wasm_pkg_core::wit::OutputType;
use wasmcloud_core::parse_wit_package_name;

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
    let project_config = match load_config(config_path, Some(true)).await {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("failed to load project configuration: {e}");
            None
        }
    };

    // Extract/build configuration for wkg
    let mut wkg_config = match project_config {
        Some(ProjectConfig {
            ref package_config, ..
        }) => package_config.clone(),
        None => wasm_pkg_core::config::Config::load().await?,
    };
    let mut wkg_config_overrides = wkg_config.overrides.take().unwrap_or_default();

    let mut lock_file =
        load_lock_file(std::env::current_dir().context("failed to get current directory")?).await?;

    // Start building the wkg client config
    let mut wkg_client_config = common.load_config().await?;

    // If a project configuration was provided, apply any pull-related overrides
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
        for RegistryPullSourceOverride { interface, source } in pull_cfg.sources {
            let (ns, pkgs, _, _, maybe_version) = parse_wit_package_name(&interface)?;
            let version_suffix = maybe_version.map(|v| format!("@{v}")).unwrap_or_default();

            match source {
                // Local files can be used by adding them to the config
                RegistryPullSource::LocalPath(_) => {
                    let path = source.resolve_file_path(&wasmcloud_toml_dir).await?;
                    if !tokio::fs::try_exists(&path).await.with_context(|| {
                        format!(
                            "failed to check for registry pull source local path [{}]",
                            path.display()
                        )
                    })? {
                        bail!(
                            "registry pull source path [{}] does not exist",
                            path.display()
                        );
                    }

                    match (ns, pkgs.as_slice()) {
                        // namespace-level override
                        (ns, []) => {
                            wkg_config_overrides.insert(
                                ns,
                                Override {
                                    path: Some(path),
                                    version: None,
                                },
                            );
                        }
                        // package-level override
                        (ns, packages) => {
                            for pkg in packages {
                                wkg_config_overrides.insert(
                                    format!("{ns}:{pkg}"),
                                    Override {
                                        path: Some(path.clone()),
                                        version: None,
                                    },
                                );
                            }
                        }
                    }
                }
                RegistryPullSource::Builtin => bail!("no builtins are supported"),
                RegistryPullSource::RemoteHttp(_) => {
                    bail!("remote HTTP WIT files/archives are not yet supported")
                }
                RegistryPullSource::RemoteGit(_) => {
                    bail!("remote Git repositories are not yet supported")
                }
                // All other registry pull sources should be directly convertible to `RegistryMapping`s
                rps @ (RegistryPullSource::RemoteOci(_)
                | RegistryPullSource::RemoteHttpWellKnown(_)) => {
                    match (ns, pkgs.as_slice()) {
                        // namespace-level override
                        (ns, []) => {
                            wkg_client_config.set_namespace_registry(
                                format!("{ns}{version_suffix}").try_into()?,
                                rps.try_into()?,
                            );
                        }
                        // package-level override
                        (ns, packages) => {
                            for pkg in packages {
                                wkg_client_config.set_package_registry_override(
                                    PackageRef::new(
                                        ns.clone().try_into()?,
                                        pkg.to_string().try_into()?,
                                    ),
                                    rps.clone().try_into()?,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // Re-add the modified local wkg config overrides
    wkg_config.overrides = Some(wkg_config_overrides);

    // Build the wkg client
    let wkg_client = common.get_client_with_config(wkg_client_config).await?;

    // NOTE: Monkey patch & fetch logging performs wkg fetching and placement as a side effect
    monkey_patch_fetch_logging(wkg_config, dir, &mut lock_file, wkg_client).await?;

    // Now write out the lock file since everything else succeeded
    lock_file.write().await?;

    Ok("Dependencies fetched".into())
}
