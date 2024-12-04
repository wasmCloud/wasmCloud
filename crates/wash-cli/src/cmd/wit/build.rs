use std::path::PathBuf;

use anyhow::Context;
use clap::Args;
use wash_lib::build::load_lock_file;
use wash_lib::cli::{CommandOutput, CommonPackageArgs};
use wash_lib::parser::load_config;

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
    let client = common.get_client().await?;
    // Attempt to load wasmcloud.toml. If it doesn't work, attempt to load wkg.toml
    let wkg_config = if let Ok(proj) = load_config(config_path, Some(true)).await {
        proj.package_config
    } else {
        wasm_pkg_core::config::Config::load().await?
    };

    let mut lock_file =
        load_lock_file(std::env::current_dir().context("failed to get current directory")?).await?;

    // Build the WIT package
    let (pkg_ref, version, bytes) =
        wit::build_package(&wkg_config, dir, &mut lock_file, client).await?;

    // Resolve the output path
    let output_path = match output_file {
        Some(path) => path,
        None => {
            let mut file_name = pkg_ref.to_string();
            if let Some(ref version) = version {
                file_name.push_str(&format!("@{version}"));
            }
            file_name.push_str(".wasm");
            PathBuf::from(file_name)
        }
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
