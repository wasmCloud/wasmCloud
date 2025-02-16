//! `wash config` related (sub)commands

use clap::Subcommand;
use crate::lib::cli::{input_vec_to_hashmap, CliConnectionOpts, CommandOutput, OutputKind};

use crate::cmd;
use crate::secrets::ensure_not_secret;

pub(crate) mod delete;
pub(crate) mod get;
pub(crate) mod put;

#[derive(Debug, Clone, Subcommand)]
#[allow(clippy::enum_variant_names)]
pub enum ConfigCliCommand {
    /// Put a named configuration
    #[clap(name = "put", alias = "create", about = "Put named configuration")]
    PutCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        /// The name of the configuration to put
        #[clap(name = "name")]
        name: String,
        /// The configuration values to put, in the form of `key=value`. Can be specified multiple times, but must be specified at least once.
        #[clap(name = "config_value", required = true)]
        config_values: Vec<String>,
    },
    /// Get a named configuration
    #[clap(name = "get")]
    GetCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        /// The name of the configuration to get
        #[clap(name = "name")]
        name: String,
    },
    /// Delete a named configuration
    #[clap(name = "del", alias = "delete")]
    DelCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        /// The name of the configuration to delete
        #[clap(name = "name")]
        name: String,
    },
}

/// Handle any `wash config` prefixed (sub)command
pub async fn handle_command(
    command: ConfigCliCommand,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    match command {
        ConfigCliCommand::PutCommand {
            opts,
            name,
            config_values,
        } => {
            ensure_not_secret(&name)?;
            cmd::config::put::invoke(
                opts,
                &name,
                input_vec_to_hashmap(config_values)?,
                output_kind,
            )
            .await
        }
        ConfigCliCommand::GetCommand { opts, name } => {
            ensure_not_secret(&name)?;
            cmd::config::get::invoke(opts, &name, output_kind).await
        }
        ConfigCliCommand::DelCommand { opts, name } => {
            ensure_not_secret(&name)?;
            cmd::config::delete::invoke(opts, &name, output_kind).await
        }
    }
}
