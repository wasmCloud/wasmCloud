use std::collections::HashMap;

use anyhow::{bail, Result};
use serde_json::json;
use wash_lib::cli::link::{
    create_link, delete_link, query_links, LinkCommand, LinkDelCommand, LinkPutCommand,
    LinkQueryCommand,
};
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::id::validate_contract_id;
use wasmcloud_control_interface::LinkDefinition;

use crate::appearance::spinner::Spinner;
use crate::ctl::{link_del_output, links_table};

/// Generate output for link put command
pub fn link_put_output(
    actor_id: impl AsRef<str>,
    provider_id: impl AsRef<str>,
    failure: Option<String>,
) -> Result<CommandOutput> {
    let actor_id = actor_id.as_ref();
    let provider_id = provider_id.as_ref();
    match failure {
        None => {
            let mut map = HashMap::new();
            map.insert("actor_id".to_string(), json!(actor_id));
            map.insert("provider_id".to_string(), json!(provider_id));
            Ok(CommandOutput::new(
                format!("Published link ({actor_id}) <-> ({provider_id}) successfully"),
                map,
            ))
        }
        Some(f) => bail!("Error advertising link: {}", f),
    }
}

/// Generate output for the link query command
pub fn link_query_output(list: Vec<LinkDefinition>) -> CommandOutput {
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
            actor_id,
            contract_id,
            link_name,
            opts,
        }) => {
            let link_name = link_name.clone().unwrap_or_else(|| "default".to_string());

            validate_contract_id(&contract_id)?;

            sp.update_spinner_message(format!(
                "Deleting link for {} on {} ({}) ... ",
                actor_id, contract_id, link_name,
            ));

            let failure = delete_link(opts.try_into()?, &contract_id, &actor_id, &link_name)
                .await
                .map_or_else(|e| Some(format!("{e}")), |_| None);

            link_del_output(&actor_id, &contract_id, &link_name, failure)?
        }
        LinkCommand::Put(LinkPutCommand {
            opts,
            contract_id,
            actor_id,
            provider_id,
            link_name,
            values,
        }) => {
            validate_contract_id(&contract_id)?;

            sp.update_spinner_message(format!(
                "Defining link between {actor_id} and {provider_id} ... ",
            ));

            let link_name = link_name.unwrap_or_else(|| "default".to_string());

            let failure = create_link(
                opts.try_into()?,
                &contract_id,
                &actor_id,
                &provider_id,
                &link_name,
                &values,
            )
            .await
            .map_or_else(|e| Some(format!("{e}")), |_| None);

            link_put_output(&actor_id, &provider_id, failure)?
        }
        LinkCommand::Query(LinkQueryCommand { opts }) => {
            sp.update_spinner_message("Querying Links ... ".to_string());
            let result = query_links(opts.try_into()?).await?;
            link_query_output(result)
        }
    };

    Ok(out)
}
