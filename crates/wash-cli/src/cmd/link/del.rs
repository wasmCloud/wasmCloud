//! Functionality enabling the `wash link del` subcommand

use std::collections::HashMap;

use anyhow::{anyhow, bail, ensure, Result};
use serde_json::json;
use wash_lib::cli::link::{delete_link, get_links, LinkDelCommand};
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::config::WashConnectionOptions;

use crate::appearance::spinner::Spinner;

/// Invoke `wash link del` subcommand
pub async fn invoke(
    LinkDelCommand {
        source_id,
        link_name,
        wit_namespace: namespace,
        wit_package: package,
        opts,
    }: LinkDelCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
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

    link_del_output(&source_id, &link_name, &namespace, &package, failure)
}

fn link_del_output(
    source_id: &str,
    link_name: &str,
    wit_namespace: &str,
    wit_package: &str,
    failure: Option<String>,
) -> Result<CommandOutput> {
    match failure {
        None => {
            let mut map = HashMap::new();
            map.insert("source_id".to_string(), json!(source_id));
            map.insert("wit_namespace".to_string(), json!(wit_namespace));
            map.insert("wit_package".to_string(), json!(wit_package));
            map.insert("link_name".to_string(), json!(link_name));
            Ok(CommandOutput::new(
                format!(
                    "Deleted link for {source_id} on {wit_namespace}:{wit_package} ({link_name}) successfully"
                ),
                map,
            ))
        }
        Some(f) => bail!("Error deleting link: {}", f),
    }
}
