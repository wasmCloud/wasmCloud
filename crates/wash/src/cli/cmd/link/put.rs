//! Functionality enabling the `wash link put` subcommand

use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};
use serde_json::json;
use crate::lib::cli::link::{put_link, LinkPutCommand};
use crate::lib::cli::{CommandOutput, OutputKind};
use wasmcloud_control_interface::Link;

use crate::appearance::spinner::Spinner;

/// Invoke `wash link put` subcommand
pub async fn invoke(
    LinkPutCommand {
        opts,
        source_id,
        target,
        link_name,
        wit_namespace,
        wit_package,
        interfaces,
        source_config,
        target_config,
    }: LinkPutCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    sp.update_spinner_message(format!("Defining link {source_id} -> {target} ... ",));

    let name = link_name.unwrap_or_else(|| "default".to_string());

    let failure = put_link(
        opts.try_into()?,
        Link::builder()
            .source_id(&source_id)
            .target(&target)
            .name(&name)
            .wit_namespace(&wit_namespace)
            .wit_package(&wit_package)
            .interfaces(interfaces)
            .source_config(source_config)
            .target_config(target_config)
            .build()
            .map_err(|e| anyhow!(e).context("failed to build link"))?,
    )
    .await
    .map_or_else(
        |e| Some(format!("{e}")),
        // If the operation was unsuccessful, return the error message
        |ctl_response| (!ctl_response.succeeded()).then_some(ctl_response.message().to_string()),
    );

    link_put_output(&source_id, &target, failure)
}

/// Generate output for `wash link put` command
fn link_put_output(
    source_id: impl AsRef<str>,
    target: impl AsRef<str>,
    failure: Option<String>,
) -> Result<CommandOutput> {
    let source_id = source_id.as_ref();
    let target = target.as_ref();
    match failure {
        None => {
            let mut map = HashMap::new();
            map.insert("source_id".to_string(), json!(source_id));
            map.insert("target".to_string(), json!(target));
            Ok(CommandOutput::new(
                format!("Published link ({source_id}) -> ({target}) successfully"),
                map,
            ))
        }
        Some(f) => bail!("Error putting link: {f}"),
    }
}
