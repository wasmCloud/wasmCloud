// This is pretty much a copy of the wkg wit subcommand adapted for wash
use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, Subcommand};
use wash_lib::{
    build::{load_lock_file, monkey_patch_fetch_logging},
    cli::{CommandOutput, CommonPackageArgs},
    parser::load_config,
};
use wasm_pkg_client::{PublishOpts, Registry};
use wasm_pkg_core::wit::{self, OutputType};

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
    Deps(DepsArgs),
    /// Publish a WIT package to a registry. This will automatically infer the package name from the
    /// WIT package.
    Publish(PublishArgs),
}

impl WitCommand {
    pub async fn run(self) -> anyhow::Result<CommandOutput> {
        match self {
            WitCommand::Build(args) => args.run().await,
            WitCommand::Deps(args) => args.run().await,
            WitCommand::Publish(args) => args.run().await,
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

    /// Path to the wasmcloud.toml file or parent folder to use for building
    #[clap(short = 'p', long = "config-path")]
    config_path: Option<PathBuf>,
}

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

#[derive(Args, Debug, Clone)]
pub struct PublishArgs {
    /// The file to publish
    file: PathBuf,

    /// The registry domain to use. Overrides configuration file(s).
    #[arg(long = "wit-registry", env = "WASH_WIT_REGISTRY")]
    registry: Option<Registry>,

    #[command(flatten)]
    common: CommonPackageArgs,
}

impl BuildArgs {
    pub async fn run(self) -> anyhow::Result<CommandOutput> {
        let client = self.common.get_client().await?;
        // Attempt to load wasmcloud.toml. If it doesn't work, attempt to load wkg.toml
        let wkg_config = if let Ok(proj) = load_config(self.config_path, Some(true)).await {
            proj.package_config
        } else {
            wasm_pkg_core::config::Config::load().await?
        };
        let mut lock_file =
            load_lock_file(std::env::current_dir().context("failed to get current directory")?)
                .await?;
        let (pkg_ref, version, bytes) =
            wit::build_package(&wkg_config, self.dir, &mut lock_file, client).await?;
        let output_path = if let Some(path) = self.output_file {
            path
        } else {
            let mut file_name = pkg_ref.to_string();
            if let Some(ref version) = version {
                file_name.push_str(&format!("@{version}"));
            }
            file_name.push_str(".wasm");
            PathBuf::from(file_name)
        };

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
}

impl DepsArgs {
    pub async fn run(self) -> anyhow::Result<CommandOutput> {
        let client = self.common.get_client().await?;
        // Attempt to load wasmcloud.toml. If it doesn't work, attempt to load wkg.toml
        let wkg_config = if let Ok(proj) = load_config(self.config_path, Some(true)).await {
            proj.package_config
        } else {
            wasm_pkg_core::config::Config::load().await?
        };
        let mut lock_file =
            load_lock_file(std::env::current_dir().context("failed to get current directory")?)
                .await?;
        monkey_patch_fetch_logging(wkg_config, self.dir, &mut lock_file, client).await?;
        // Now write out the lock file since everything else succeeded
        lock_file.write().await?;
        Ok("Dependencies fetched".into())
    }
}

impl PublishArgs {
    pub async fn run(self) -> anyhow::Result<CommandOutput> {
        let client = self.common.get_client().await?;

        let (package, version) = client
            .client()?
            .publish_release_file(
                &self.file,
                PublishOpts {
                    registry: self.registry,
                    ..Default::default()
                },
            )
            .await?;

        Ok(CommandOutput::new(
            format!("Published {}@{}", package, version),
            [
                ("package".to_string(), package.to_string().into()),
                ("version".to_string(), version.to_string().into()),
            ]
            .into(),
        ))
    }
}
