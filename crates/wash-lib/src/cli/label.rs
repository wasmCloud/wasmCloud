use anyhow::{bail, Result};
use clap::Parser;

use crate::{
    common::{boxed_err_to_anyhow, find_host_id},
    config::WashConnectionOptions,
};

use super::{CliConnectionOpts, CommandOutput};

#[derive(Debug, Clone, Parser)]
pub struct LabelHostCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// ID of host to update the actor on. If a non-ID is provided, the host will be selected based
    /// on matching the prefix of the ID or the friendly name and will return an error if more than
    /// one host matches.
    #[clap(name = "host-id")]
    pub host_id: String,

    /// Delete the label, instead of adding it
    #[clap(long = "delete", default_value = "false")]
    pub delete: bool,

    /// Host label in the form of a `[key]=[value]` pair, e.g. "cloud=aws". When `--delete` is set, only the key is provided
    #[clap(name = "label", alias = "label")]
    pub label: String,
}

pub async fn handle_label_host(cmd: LabelHostCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let (host_id, friendly_name) = find_host_id(&cmd.host_id, &client).await?;

    let friendly_name = if friendly_name.is_empty() {
        host_id.to_string()
    } else {
        friendly_name
    };

    let (key, value) = match cmd.label.split_once('=') {
        Some((k, v)) => (k, v),
        None => (cmd.label.as_str(), ""),
    };

    if cmd.delete {
        let ack = client
            .delete_label(&host_id, key)
            .await
            .map_err(boxed_err_to_anyhow)?;
        if !ack.accepted {
            bail!("Operation failed: {}", ack.error);
        }

        Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Host `{}` unlabeled with `{}`", friendly_name, key),
        ))
    } else {
        if value.is_empty() {
            bail!("No value provided");
        }

        let ack = client
            .put_label(&host_id, key, value)
            .await
            .map_err(boxed_err_to_anyhow)?;
        if !ack.accepted {
            bail!("Operation failed: {}", ack.error);
        }

        Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Host `{}` labeled with `{}={}`", friendly_name, key, value),
        ))
    }
}
