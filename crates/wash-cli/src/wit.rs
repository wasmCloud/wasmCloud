// This is pretty much a copy of the wkg wit subcommand adapted for wash
use std::path::PathBuf;

use anyhow::Ok;
use clap::{Args, Subcommand};
use wash_lib::cli::{CommandOutput, CommonPackageArgs};
use wasm_pkg_core::{
    lock::LockFile,
    wit::{self, OutputType},
};

/// Commands for interacting with wit
#[derive(Debug, Subcommand, Clone)]
pub enum WitCommand {
    /// Build a WIT package from a directory. By default, this will fetch all dependencies needed
    /// and encode them in the WIT package. This will generate a lock file that can be used to fetch
    /// the dependencies in the future.
    Build(BuildArgs),
    /// Fetch dependencies for a component. This will read the package containing the world(s) you
    /// have defined in the given wit directory (`wit` by default). It will then fetch the
    /// dependencies and write them to the `deps` directory along with a lock file. If no lock file
    /// exists, it will fetch all dependencies. If a lock file exists, it will fetch any
    /// dependencies that are not in the lock file and update the lock file.
    Fetch(FetchArgs),
}

impl WitCommand {
    pub async fn run(self) -> anyhow::Result<CommandOutput> {
        match self {
            WitCommand::Build(args) => args.run().await,
            WitCommand::Fetch(args) => args.run().await,
        }
    }
}

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
}

#[derive(Debug, Args, Clone)]
pub struct FetchArgs {
    /// The directory containing the WIT files to fetch dependencies for.
    #[clap(short = 'd', long = "wit-dir", default_value = "wit")]
    pub dir: PathBuf,

    /// The desired output type of the dependencies. Valid options are "wit" or "wasm" (wasm is the
    /// WIT package binary format).
    #[clap(short = 't', long = "type")]
    pub output_type: Option<OutputType>,

    #[clap(flatten)]
    pub common: CommonPackageArgs,
}

impl BuildArgs {
    pub async fn run(self) -> anyhow::Result<CommandOutput> {
        let client = self.common.get_client().await?;
        let wkg_config = wasm_pkg_core::config::Config::load().await?;
        let mut lock_file = LockFile::load(false).await?;
        let (pkg_ref, version, bytes) =
            wit::build_package(&wkg_config, self.dir, &mut lock_file, client).await?;
        let output_path = if let Some(path) = self.output_file {
            path
        } else {
            let mut file_name = pkg_ref.to_string();
            if let Some(version) = version {
                file_name.push_str(&format!("@{version}"));
            }
            file_name.push_str(".wasm");
            PathBuf::from(file_name)
        };

        tokio::fs::write(&output_path, bytes).await?;
        // Now write out the lock file since everything else succeeded
        lock_file.write().await?;
        Ok(format!("WIT package written to {}", output_path.display()).into())
    }
}

impl FetchArgs {
    pub async fn run(self) -> anyhow::Result<CommandOutput> {
        let client = self.common.get_client().await?;
        let wkg_config = wasm_pkg_core::config::Config::load().await?;
        let mut lock_file = LockFile::load(false).await?;
        wit::fetch_dependencies(
            &wkg_config,
            self.dir,
            &mut lock_file,
            client,
            self.output_type.unwrap_or_default(),
        )
        .await?;
        // Now write out the lock file since everything else succeeded
        lock_file.write().await?;
        Ok("Dependencies fetched".into())
    }
}
