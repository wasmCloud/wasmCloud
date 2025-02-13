use std::collections::HashMap;

use anyhow::Result;
use clap::Parser;
use serde_json::json;
use tracing::{error, warn};

use crate::lib::{
    common::{boxed_err_to_anyhow, find_host_id},
    config::WashConnectionOptions,
};

use super::{CliConnectionOpts, CommandOutput};

#[derive(Debug, Clone, Parser)]
pub struct LabelHostCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// ID of host to update the component on. If a non-ID is provided, the host will be selected based
    /// on matching the prefix of the ID or the friendly name and will return an error if more than
    /// one host matches.
    #[clap(name = "host-id")]
    pub host_id: String,

    /// Delete the label, instead of adding it
    #[clap(long = "delete", default_value = "false")]
    pub delete: bool,

    /// Host label in the form of a `[key]=[value]` pair, e.g. "cloud=aws". When `--delete` is set, only the key is provided
    #[clap(name = "label", alias = "label", value_delimiter = ',')]
    pub labels: Vec<String>,
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

    let labels = cmd
        .labels
        .iter()
        .map(|orig| match orig.split_once('=') {
            Some((k, v)) => (k, v),
            None => (orig.as_str(), ""),
        })
        .collect::<Vec<(&str, &str)>>();

    // Set/Delete the provided labels
    let mut succeeded = true;
    let mut processed: Vec<(&str, &str)> = Vec::new();
    for (key, value) in &labels {
        let op = if cmd.delete {
            client
                .delete_label(&host_id, key)
                .await
                .map_err(boxed_err_to_anyhow)
        } else {
            client
                .put_label(&host_id, key, value)
                .await
                .map_err(boxed_err_to_anyhow)
        };

        match op {
            Ok(ack) => {
                if !ack.succeeded() {
                    warn!(message = ack.message(), "operation failed");
                    succeeded = false;
                    break;
                }
                processed.push((key, value));
            }
            Err(error) => {
                error!(?error, "failed to set/delete label");
                succeeded = false;
                break;
            }
        }
    }

    let output = format!(
        "Host `{friendly_name}` {} with `{}`",
        if cmd.delete { "unlabeled" } else { "labeled" },
        labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<String>>()
            .join(",")
    );

    Ok(CommandOutput::new(
        output,
        HashMap::from([
            ("success".into(), json!(succeeded)),
            ("deleted".into(), json!(cmd.delete)),
            ("processed".into(), json!(processed)),
        ]),
    ))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::LabelHostCommand;

    const HOST_ID: &str = "host-id";

    /// Ensure multiple labels work when specified multiple times
    #[test]
    fn test_label_multiple_joined() {
        #[derive(Parser, Debug)]
        struct Cmd {
            #[clap(flatten)]
            command: LabelHostCommand,
        }

        let expected_labels = vec!["key1=value1", "key2=value2"];
        let cmd: Cmd =
            Parser::try_parse_from(["label", HOST_ID, &expected_labels.join(",")]).unwrap();
        let LabelHostCommand {
            host_id,
            delete,
            labels,
            ..
        } = cmd.command;
        assert_eq!(host_id, HOST_ID);
        assert!(!delete);
        assert_eq!(labels, expected_labels);
    }

    /// Ensure single label works
    #[test]
    fn test_label_single() {
        #[derive(Parser, Debug)]
        struct Cmd {
            #[clap(flatten)]
            command: LabelHostCommand,
        }

        let cmd: Cmd = Parser::try_parse_from(["label", HOST_ID, "key1=value1"]).unwrap();
        let LabelHostCommand {
            host_id,
            delete,
            labels,
            ..
        } = cmd.command;
        assert_eq!(host_id, HOST_ID);
        assert!(!delete);
        assert_eq!(labels, vec!["key1=value1"]);
    }
}
