use std::path::PathBuf;

use clap::Args;
use crate::lib::build::load_lock_file;
use crate::lib::cli::{CommandOutput, CommonPackageArgs};
use crate::lib::deps::WkgFetcher;
use crate::lib::parser::load_config;

use wasm_pkg_core::wit::{self};

/// Arguments used for `wash wit build`
#[derive(Debug, Args, Clone)]
pub struct BuildArgs {
    /// The directory containing the WIT files to build.
    #[clap(short = 'd', long = "wit-dir", default_value = "wit")]
    pub dir: PathBuf,

    /// The name of the file that should be written. This can also be a full path. Defaults to the
    /// current directory with the name of the package
    #[clap(short = 'f', long = "file")]
    pub output_file: Option<PathBuf>,

    #[clap(flatten)]
    pub common: CommonPackageArgs,

    /// Path to the wasmcloud.toml file or parent folder to use for building
    #[clap(short = 'p', long = "config-path")]
    config_path: Option<PathBuf>,
}

/// Invoke `wash wit build`
pub async fn invoke(
    BuildArgs {
        dir,
        output_file,
        common,
        config_path,
    }: BuildArgs,
) -> anyhow::Result<CommandOutput> {
    // Attempt to load wasmcloud.toml. If it doesn't work, attempt to load wkg.toml
    let wkg_config = if let Ok(proj) = load_config(config_path.clone(), Some(true)).await {
        proj.package_config
    } else {
        wasm_pkg_core::config::Config::load().await?
    };

    let wkg = WkgFetcher::from_common(&common, wkg_config).await?;

    let project_cfg_dir = load_config(config_path, Some(true))
        .await
        .map(|cfg| cfg.wasmcloud_toml_dir)
        .unwrap_or(dir.clone());
    let mut lock_file = load_lock_file(&project_cfg_dir).await?;

    // Build the WIT package
    let (pkg_ref, version, bytes) = wit::build_package(
        &wkg.get_config().to_owned(),
        dir,
        &mut lock_file,
        wkg.into_client(),
    )
    .await?;

    // Resolve the output path
    let output_path = if let Some(path) = output_file { path } else {
        let mut file_name = pkg_ref.to_string();
        if let Some(ref version) = version {
            file_name.push_str(&format!("@{version}"));
        }
        file_name.push_str(".wasm");
        PathBuf::from(file_name)
    };

    // Write out the WIT tot the specified path
    tokio::fs::write(&output_path, bytes).await?;

    // Now write out the lock file since everything else succeeded
    lock_file.write().await?;

    Ok(CommandOutput::new(
        format!("WIT package written to {}", output_path.display()),
        [
            ("path".to_string(), serde_json::to_value(output_path)?),
            ("package".to_string(), pkg_ref.to_string().into()),
            ("version".to_string(), version.map(|v| v.to_string()).into()),
        ]
        .into(),
    ))
}
