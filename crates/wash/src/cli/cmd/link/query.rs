//! Functionality enabling the `wash link get` subcommand

use std::collections::HashMap;

use anyhow::Result;
use serde_json::json;
use crate::lib::cli::link::{get_links, LinkQueryCommand};
use crate::lib::cli::{CommandOutput, OutputKind};
use wasmcloud_control_interface::Link;

use crate::appearance::spinner::Spinner;
use crate::ctl::links_table;

/// Generate output for the `wash link query` command
pub fn link_query_output(list: Vec<Link>) -> CommandOutput {
    let map = HashMap::from([("links".to_string(), json!(list))]);
    CommandOutput::new(links_table(list), map)
}

/// Invoke `wash link del` subcommand
pub async fn invoke(
    LinkQueryCommand { opts }: LinkQueryCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    sp.update_spinner_message("Querying Links ... ".to_string());
    let result = get_links(opts.try_into()?).await?;
    Ok(link_query_output(result))
}
