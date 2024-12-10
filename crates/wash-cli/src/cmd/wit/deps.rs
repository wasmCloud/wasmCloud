use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use async_compression::tokio::bufread::GzipDecoder;
use clap::Args;
use futures::TryStreamExt as _;
use reqwest::Url;
use walkdir::WalkDir;
use wash_lib::build::{load_lock_file, monkey_patch_fetch_logging};
use wash_lib::cli::{CommandOutput, CommonPackageArgs};
use wash_lib::common::clone_git_repo;
use wash_lib::parser::{
    load_config, CommonConfig, ProjectConfig, RegistryConfig, RegistryPullConfig,
    RegistryPullSource, RegistryPullSourceOverride,
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

    let mut lock_file =
        load_lock_file(std::env::current_dir().context("failed to get current directory")?).await?;

    // Start building the wkg client config
    let mut wkg_client_config = common.load_config().await?;

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
        resolve_extended_pull_configs(
            &pull_cfg,
            &wasmcloud_toml_dir,
            &mut wkg_config,
            &mut wkg_client_config,
        )
        .await?;
    }

    // Build the wkg client
    let wkg_client = common.get_client_with_config(wkg_client_config).await?;

    // NOTE: Monkey patch & fetch logging performs wkg fetching and placement as a side effect
    monkey_patch_fetch_logging(wkg_config, dir, &mut lock_file, wkg_client).await?;

    // Now write out the lock file since everything else succeeded
    lock_file.write().await?;

    Ok("Dependencies fetched".into())
}

/// Enable extended pull configurations for wkg config
async fn resolve_extended_pull_configs(
    pull_cfg: &RegistryPullConfig,
    wasmcloud_toml_dir: impl AsRef<Path>,
    wkg_config: &mut wasm_pkg_core::config::Config,
    wkg_client_config: &mut wasm_pkg_client::Config,
) -> Result<()> {
    let mut wkg_config_overrides = wkg_config.overrides.get_or_insert_default();

    for RegistryPullSourceOverride { target, source } in pull_cfg.sources.iter() {
        let (ns, pkgs, _, _, maybe_version) = parse_wit_package_name(target)?;
        let version_suffix = maybe_version.map(|v| format!("@{v}")).unwrap_or_default();

        match source {
            // Local files can be used by adding them to the config
            RegistryPullSource::LocalPath(_) => {
                let path = source
                    .resolve_file_path(wasmcloud_toml_dir.as_ref())
                    .await?;
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
            RegistryPullSource::RemoteHttp(s) => {
                let url = Url::parse(s)
                    .with_context(|| format!("invalid registry pull source url [{s}]"))?;
                let tempdir = tempfile::tempdir()
                    .with_context(|| format!("failed to create temp dir for downloading [{url}]"))?
                    .into_path();
                let output_path = tempdir.join("unpacked");
                let http_client = wash_lib::start::get_download_client()?;
                let req = http_client.get(url.clone()).send().await.with_context(|| {
                    format!("failed to retrieve WIT output from URL [{}]", &url)
                })?;
                let mut archive = tokio_tar::Archive::new(GzipDecoder::new(
                    tokio_util::io::StreamReader::new(req.bytes_stream().map_err(|e| {
                        std::io::Error::other(format!(
                            "failed to receive byte stream while downloading from URL [{}]: {e}",
                            &url
                        ))
                    })),
                ));
                archive.unpack(&output_path).await.with_context(|| {
                    format!("failed to unpack archive downloaded from URL [{}]", &url)
                })?;

                // Find the first nested directory named 'wit', if present
                let output_wit_dir = find_wit_folder_in_path(&output_path).await?;

                // Set the local override for the namespaces and/or packages
                match (ns, pkgs.as_slice()) {
                    // namespace-level override
                    (ns, []) => {
                        wkg_config_overrides.insert(
                            ns,
                            Override {
                                path: Some(output_wit_dir),
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
                                    path: Some(output_wit_dir.clone()),
                                    version: None,
                                },
                            );
                        }
                    }
                }
            }
            RegistryPullSource::RemoteGit(s) => {
                let url = Url::parse(s)
                    .with_context(|| format!("invalid registry pull source url [{s}]"))?;
                let query_pairs = url.query_pairs().collect::<HashMap<_, _>>();
                let tempdir = tempfile::tempdir()
                    .with_context(|| format!("failed to create temp dir for downloading [{url}]"))?
                    .into_path();

                clone_git_repo(
                    None,
                    &tempdir,
                    s.into(),
                    query_pairs
                        .get("subfolder")
                        .map(|s| String::from(s.clone())),
                    query_pairs.get("branch").map(|s| String::from(s.clone())),
                )
                .await
                .with_context(|| format!("failed to clone repo for pull source git repo [{s}]",))?;

                // Find the first nested directory named 'wit', if present
                let output_wit_dir = find_wit_folder_in_path(&tempdir).await?;

                // Set the local override for the namespaces and/or packages
                match (ns, pkgs.as_slice()) {
                    // namespace-level override
                    (ns, []) => {
                        wkg_config_overrides.insert(
                            ns,
                            Override {
                                path: Some(output_wit_dir),
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
                                    path: Some(output_wit_dir.clone()),
                                    version: None,
                                },
                            );
                        }
                    }
                }
            }
            // All other registry pull sources should be directly convertible to `RegistryMapping`s
            rps @ (RegistryPullSource::RemoteOci(_)
            | RegistryPullSource::RemoteHttpWellKnown(_)) => {
                let registry = rps.clone().try_into()?;
                match (ns, pkgs.as_slice()) {
                    // namespace-level override
                    (ns, []) => {
                        wkg_client_config.set_namespace_registry(
                            format!("{ns}{version_suffix}").try_into()?,
                            registry,
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
                                registry.clone(),
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Find a folder named 'wit' inside a given dir, if present, otherwise return the path itself
async fn find_wit_folder_in_path(dir: impl AsRef<Path>) -> Result<PathBuf> {
    let dir = dir.as_ref().to_path_buf();
    // Find the first nested directory named 'wit', if present
    tokio::task::spawn_blocking(move || {
        if let Some(path) = WalkDir::new(&dir)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .find(|e| e.file_name() == "wit")
            .map(walkdir::DirEntry::into_path)
        {
            path
        } else {
            dir
        }
    })
    .await
    .context("failed to resolve folder with WIT in downloaded archive")
}
