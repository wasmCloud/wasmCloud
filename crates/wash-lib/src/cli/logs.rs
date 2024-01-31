use anyhow::{bail, Result};
use clap::Parser;

use crate::{
    cli::{CliConnectionOpts, CommandOutput},
    common::{boxed_err_to_anyhow, find_host_id},
    config::WashConnectionOptions,
};

#[derive(Clone, Debug, Parser)]
pub struct GetLoggingConfigCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// ID of host to update the actor on. If a non-ID is provided, the host will be selected based
    /// on matching the prefix of the ID or the friendly name and will return an error if more than
    /// one host matches.
    #[clap(name = "host-id")]
    pub host_id: String,
}

#[derive(Clone, Debug, Parser)]
pub struct SetLoggingConfigCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// ID of host to update the actor on. If a non-ID is provided, the host will be selected based
    /// on matching the prefix of the ID or the friendly name and will return an error if more than
    /// one host matches.
    #[clap(name = "host-id")]
    pub host_id: String,

    /// Logging level to set
    #[clap(short = 'l', long = "level")]
    pub level: String,
}

#[derive(Debug, Clone, Parser)]
pub enum LogsCommand {
    /// Get the logging config for a host
    #[clap(name = "get")]
    Get(GetLoggingConfigCommand),

    /// Set the logging config for a host
    #[clap(name = "set")]
    Set(SetLoggingConfigCommand),
}

pub async fn handle_get_logging_config(cmd: GetLoggingConfigCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let (host_id, friendly_name) = find_host_id(&cmd.host_id, &client).await?;

    let friendly_name = if friendly_name.is_empty() {
        host_id.to_string()
    } else {
        friendly_name
    };

    let config = client
        .get_logging_config(&host_id)
        .await
        .map_err(boxed_err_to_anyhow)?;

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!(
            "Host `{}` configured with log level `{}`",
            friendly_name, config.level
        ),
    ))
}

pub async fn handle_set_logging_config(cmd: SetLoggingConfigCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let (host_id, friendly_name) = find_host_id(&cmd.host_id, &client).await?;

    let friendly_name = if friendly_name.is_empty() {
        host_id.to_string()
    } else {
        friendly_name
    };

    let ack = client
        .set_logging_config(&host_id, cmd.level.clone().try_into()?)
        .await
        .map_err(boxed_err_to_anyhow)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!(
            "Host `{}` configured with log level `{}`",
            friendly_name, cmd.level
        ),
    ))
}
