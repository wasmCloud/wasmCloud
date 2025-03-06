//! Functionality enabling the `wash link del` subcommand

use std::collections::HashMap;

use anyhow::{anyhow, bail, ensure, Context as _, Result};
use serde_json::json;
use crate::lib::cli::link::{delete_link, get_links, LinkDelCommand};
use crate::lib::cli::{CommandOutput, OutputKind};
use crate::lib::config::WashConnectionOptions;
use crate::lib::generate::interactive::prompt_for_choice;
use crate::lib::generate::project_variables::StringEntry;

use crate::appearance::spinner::Spinner;

/// Invoke `wash link del` subcommand
pub async fn invoke(
    LinkDelCommand {
        source_id,
        link_name,
        wit_namespace: namespace,
        wit_package: package,
        opts,
        all,
        force,
    }: LinkDelCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = opts.try_into()?;

    // If the user has chosen to delete all links, but *did not* force, then prompt
    if all {
        let delete_links = |wco: WashConnectionOptions| async move {
            let sp: Spinner = Spinner::new(&output_kind)?;
            let links = get_links(wco.clone())
                .await
                .context("failed to retrieve links")?;
            let mut deleted_links = Vec::with_capacity(links.len());
            for link in &links {
                sp.update_spinner_message(format!(
                    "Deleting link for {} on {}:{} ({}) ... ",
                    link.source_id(),
                    link.wit_namespace(),
                    link.wit_package(),
                    link.name(),
                ));
                if let Err(e) = delete_link(
                    wco.clone(),
                    link.source_id(),
                    link.name(),
                    link.wit_namespace(),
                    link.wit_package(),
                )
                .await
                .context("failed to delete link, aborting delete all operation")
                {
                    return Ok::<CommandOutput, anyhow::Error>(CommandOutput::new(
                        format!("Deleted some {} links successfully", links.len(),),
                        HashMap::from([
                            ("error".into(), json!(e.to_string())),
                            (
                                "deleted".into(),
                                json!(deleted_links.into_iter().collect::<Vec<_>>()),
                            ),
                        ]),
                    ));
                }
                deleted_links.push(link);
            }
            sp.finish_and_clear();
            Ok::<CommandOutput, anyhow::Error>(CommandOutput::new(
                format!("Deleted all links ({}) successfully", links.len(),),
                HashMap::from([(
                    "deleted".into(),
                    json!(deleted_links.into_iter().collect::<Vec<_>>()),
                )]),
            ))
        };

        if force {
            return delete_links(wco.clone())
                .await
                .context("failed to delete all links");
        } else {
            match prompt_for_choice(
                &StringEntry {
                    default: Some("cancel".to_string()),
                    choices: Some(vec!["Delete all links".to_string(), "Cancel".to_string()]),
                    regex: None,
                },
                "Are you sure you want to delete all links in the cluster? (this action cannot be reversed)",
            ) {
                Ok(0) => return delete_links(wco.clone()).await.context("failed to delete all links"),
                Ok(1) => bail!("Link deletion cancelled"),
                _ => unreachable!("unexpected choice received"),
            }
        }
    }

    let sp: Spinner = Spinner::new(&output_kind)?;
    let package = package.context("missing required argument package")?;
    let source_id = source_id.context("missing required argument source_id")?;
    let namespace = namespace.context("missing required argument namespace")?;

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
            .count();
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
