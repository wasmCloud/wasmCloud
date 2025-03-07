use clap::Subcommand;
use crate::lib::cli::CommandOutput;

mod build;
mod deps;
mod publish;

/// Commands for interacting with wit (`wash wit`)
///
/// These commands mirror the `wkg wit` subcommand, but are adapted for use with `wash`
#[derive(Debug, Subcommand, Clone)]
pub enum WitCommand {
    /// Build a WIT package from a directory.
    /// By default, this will fetch all dependencies needed and encode them
    /// in the WIT package. This will generate a lock file that can be used to fetch
    /// the dependencies in the future.
    Build(build::BuildArgs),

    /// Fetch dependencies for a component.
    ///
    /// This will read the package containing the world(s) you have defined in the
    /// given wit directory (`wit` by default). It will then fetch the
    /// dependencies and write them to the `deps` directory along with a lock file. If no lock file
    /// exists, it will fetch all dependencies. If a lock file exists, it will fetch any
    /// dependencies that are not in the lock file and update the lock file.
    #[clap(alias = "fetch")]
    Deps(deps::DepsArgs),

    /// Publish a WIT package to a registry.
    /// This will automatically infer the package name from the WIT package.
    Publish(publish::PublishArgs),
}

/// Handle the `wash wit` subcommand
pub async fn handle_command(cmd: WitCommand) -> anyhow::Result<CommandOutput> {
    match cmd {
        WitCommand::Build(args) => build::invoke(args).await,
        WitCommand::Deps(args) => deps::invoke(args).await,
        WitCommand::Publish(args) => publish::invoke(args).await,
    }
}
