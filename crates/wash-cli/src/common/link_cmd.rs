use std::collections::HashMap;

use anyhow::{bail, Result};
use serde_json::json;
use wash_lib::cli::link::{
    delete_link, get_links, put_link, LinkCommand, LinkDelCommand, LinkPutCommand, LinkQueryCommand,
};
use wash_lib::cli::{CommandOutput, OutputKind};
use wasmcloud_control_interface::InterfaceLinkDefinition;

use crate::appearance::spinner::Spinner;
use crate::ctl::{link_del_output, links_table};

/// Generate output for link put command
pub fn link_put_output(
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

/// Generate output for the link query command
pub fn link_query_output(list: Vec<InterfaceLinkDefinition>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("links".to_string(), json!(list));
    CommandOutput::new(links_table(list), map)
}

pub async fn handle_command(
    command: LinkCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        LinkCommand::Del(LinkDelCommand {
            source_id,
            link_name,
            wit_namespace: namespace,
            wit_package: package,
            opts,
        }) => {
            let link_name = link_name.clone().unwrap_or_else(|| "default".to_string());

            sp.update_spinner_message(format!(
                "Deleting link for {source_id} on {namespace}:{package} ({link_name}) ... ",
            ));

            let failure = delete_link(
                opts.try_into()?,
                &source_id,
                &link_name,
                &namespace,
                &package,
            )
            .await
            .map_or_else(|e| Some(format!("{e}")), |_| None);

            link_del_output(&source_id, &link_name, &namespace, &package, failure)?
        }
        LinkCommand::Put(LinkPutCommand {
            opts,
            source_id,
            target,
            link_name,
            wit_namespace,
            wit_package,
            interfaces,
            source_config,
            target_config,
        }) => {
            sp.update_spinner_message(format!("Defining link {source_id} -> {target} ... ",));

            let name = link_name.unwrap_or_else(|| "default".to_string());

            let failure = put_link(
                opts.try_into()?,
                InterfaceLinkDefinition {
                    source_id: source_id.to_string(),
                    target: target.to_string(),
                    name,
                    wit_namespace,
                    wit_package,
                    interfaces,
                    source_config,
                    target_config,
                },
            )
            .await
            .map_or_else(
                |e| Some(format!("{e}")),
                // If the operation was unsuccessful, return the error message
                |ctl_response| (!ctl_response.success).then_some(ctl_response.message),
            );

            link_put_output(&source_id, &target, failure)?
        }
        LinkCommand::Query(LinkQueryCommand { opts }) => {
            sp.update_spinner_message("Querying Links ... ".to_string());
            let result = get_links(opts.try_into()?).await?;
            link_query_output(result)
        }
    };

    Ok(out)
}
