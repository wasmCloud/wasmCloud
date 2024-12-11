use std::collections::HashMap;

use anyhow::{anyhow, bail, ensure, Result};
use serde_json::json;
use wash_lib::cli::link::{
    delete_link, get_links, put_link, LinkCommand, LinkDelCommand, LinkPutCommand, LinkQueryCommand,
};
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::config::WashConnectionOptions;
use wasmcloud_control_interface::Link;

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
pub fn link_query_output(list: Vec<Link>) -> CommandOutput {
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
            let wco: WashConnectionOptions = opts.try_into()?;

            // If the link name is not specified, and multiple links are similar in other ways
            // make deleting the link an error, as the user should likely be explicitly choosing
            // which they'd like to delete
            if link_name.is_none() {
                let similar_link_count = get_links(wco.clone())
                    .await
                    .map_err(|e| {
                        anyhow!(e).context("failed to retrieve links while checking for multiple")
                    })?
                    .into_iter()
                    .filter(|l| {
                        l.source_id() == source_id
                            && l.wit_namespace() == namespace
                            && l.wit_package() == package
                    })
                    .collect::<Vec<_>>()
                    .len();
                ensure!(
                    similar_link_count <= 1,
                    "More than one similar link found, please specify link name explicitly"
                );
            };

            let link_name = link_name.clone().unwrap_or_else(|| "default".to_string());

            sp.update_spinner_message(format!(
                "Deleting link for {source_id} on {namespace}:{package} ({link_name}) ... ",
            ));

            let failure = delete_link(wco, &source_id, &link_name, &namespace, &package)
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
                |ctl_response| {
                    (!ctl_response.succeeded()).then_some(ctl_response.message().to_string())
                },
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
