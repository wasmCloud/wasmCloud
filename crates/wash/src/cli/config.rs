use anyhow::Context as _;
use clap::{Parser, Subcommand};
use tracing::instrument;

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::{Config, generate_default_config, load_config, local_config_path},
};

/// View and manage wash configuration
#[derive(Parser, Debug, Clone)]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCommand {
    /// Initialize a new configuration file for wash
    Init {
        /// Overwrite existing configuration
        #[arg(long)]
        force: bool,
        /// Overwrite global configuration instead of project
        #[arg(long)]
        global: bool,
    },
    /// Print the current version and local directories used by wash
    Info {},
    /// Print the current configuration file for wash
    Show {},
    // TODO(#27): validate config command
    // TODO(#29): cleanup config command, to clean the dirs we use
}

impl CliCommand for ConfigArgs {
    #[instrument(level = "debug", skip_all, name = "config")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        self.command.handle(ctx).await
    }
}

impl CliCommand for ConfigCommand {
    #[instrument(level = "debug", skip_all, name = "config")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            ConfigCommand::Init { force, global } => {
                let config_path = if *global {
                    ctx.user_config_path()
                } else {
                    local_config_path(ctx.project_dir())
                };

                generate_default_config(&config_path, *force)
                    .await
                    .context("failed to initialize config")?;

                Ok(CommandOutput::ok(
                    "Configuration initialized successfully.".to_string(),
                    Some(serde_json::json!({
                        "message": "Configuration initialized successfully.",
                        "config_path": config_path.display().to_string(),
                        "success": true,
                    })),
                ))
            }
            ConfigCommand::Info {} => {
                let version = env!("CARGO_PKG_VERSION");
                let data_dir = ctx.data_dir().display().to_string();
                let cache_dir = ctx.cache_dir().display().to_string();
                let user_config_dir = ctx.config_dir().display().to_string();
                let user_config_path = ctx.user_config_path().display().to_string();
                let project_path = ctx.project_dir();
                let project_config_path = ctx.project_config_path().display().to_string();

                Ok(CommandOutput::ok(
                    format!(
                        r"
wash version: {version}
Data directory: {data_dir}
Cache directory: {cache_dir}
User Config directory: {user_config_dir}
User Config path: {user_config_path}
Project Config path: {project_config_path}
                        "
                    ),
                    Some(serde_json::json!({
                        "version": version,
                        "data_dir": data_dir,
                        "cache_dir": cache_dir,
                        "user_config_dir": user_config_dir,
                        "user_config_path": user_config_path,
                        "project_path": project_path,
                        "project_config_path": project_config_path,
                    })),
                ))
            }
            ConfigCommand::Show {} => {
                let config = load_config(
                    &ctx.user_config_path(),
                    Some(ctx.project_dir()),
                    None::<Config>,
                )?;
                Ok(CommandOutput::ok(
                    serde_yaml_ng::to_string(&config).context("failed to serialize config")?,
                    Some(serde_json::to_value(&config).context("failed to serialize config")?),
                ))
            }
        }
    }
}
