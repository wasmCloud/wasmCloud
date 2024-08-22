use std::collections::HashMap;

use anyhow::{bail, Result};
use serde_json::json;
use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table,
};
use wash_lib::{cli::CommandOutput, plugin::subcommand::Metadata};
use wasmcloud_control_interface::{Host, HostInventory, InterfaceLinkDefinition};

use crate::util::format_optional;

pub fn get_hosts_output(hosts: Vec<Host>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("hosts".to_string(), json!(hosts));
    CommandOutput::new(hosts_table(hosts), map)
}

pub fn get_host_inventories_output(invs: Vec<HostInventory>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("inventories".to_string(), json!(invs));
    CommandOutput::new(host_inventories_table(invs), map)
}

pub fn get_claims_output(claims: Vec<HashMap<String, String>>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("claims".to_string(), json!(claims));
    CommandOutput::new(claims_table(claims), map)
}

pub fn link_del_output(
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

/// Helper function to transform a LinkDefinitionList into a table string for printing
pub fn links_table(list: Vec<InterfaceLinkDefinition>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Source ID", 1, Alignment::Left),
        TableCell::new_with_alignment("Target", 1, Alignment::Left),
        TableCell::new_with_alignment("WIT", 1, Alignment::Left),
        TableCell::new_with_alignment("Interfaces", 1, Alignment::Left),
    ]));

    list.iter().for_each(|l| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(l.source_id.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(l.target.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(
                format!("{}:{}", l.wit_namespace, l.wit_package),
                1,
                Alignment::Left,
            ),
            TableCell::new_with_alignment(l.interfaces.join(","), 1, Alignment::Left),
        ]))
    });

    table.render()
}

/// Helper function to transform a Host list into a table string for printing
pub fn hosts_table(hosts: Vec<Host>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Host ID", 2, Alignment::Left),
        TableCell::new_with_alignment("Friendly name", 1, Alignment::Left),
        TableCell::new_with_alignment("Uptime (seconds)", 1, Alignment::Left),
    ]));
    hosts.iter().for_each(|h| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(h.id.clone(), 2, Alignment::Left),
            TableCell::new_with_alignment(h.friendly_name.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(format!("{}", h.uptime_seconds), 1, Alignment::Left),
        ]))
    });

    table.render()
}

/// Helper function to transform a HostInventory into a table string for printing
pub fn host_inventories_table(invs: Vec<HostInventory>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 3);

    invs.into_iter().for_each(|inv| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Host ID", 2, Alignment::Left),
            TableCell::new_with_alignment("Friendly name", 1, Alignment::Left),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(inv.host_id.clone(), 2, Alignment::Left),
            TableCell::new_with_alignment(inv.friendly_name.clone(), 1, Alignment::Left),
        ]));

        if !inv.labels.is_empty() {
            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                "",
                3,
                Alignment::Left,
            )]));
            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                "Host labels",
                1,
                Alignment::Left,
            )]));
            inv.labels.iter().for_each(|(k, v)| {
                table.add_row(Row::new(vec![
                    TableCell::new_with_alignment(k, 1, Alignment::Left),
                    TableCell::new_with_alignment(v, 1, Alignment::Left),
                ]))
            });
        } else {
            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                "No labels present",
                4,
                Alignment::Center,
            )]));
        }

        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "",
            4,
            Alignment::Center,
        )]));
        if !inv.components.is_empty() {
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment("Component ID", 1, Alignment::Left),
                TableCell::new_with_alignment("Name", 1, Alignment::Left),
                TableCell::new_with_alignment("Max count", 1, Alignment::Left),
            ]));
            inv.components.iter().for_each(|a| {
                let a = a.clone();
                table.add_row(Row::new(vec![
                    TableCell::new_with_alignment(a.id, 1, Alignment::Left),
                    TableCell::new_with_alignment(format_optional(a.name), 1, Alignment::Left),
                    TableCell::new_with_alignment(a.max_instances, 1, Alignment::Left),
                ]))
            });
        } else {
            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                "No components found",
                4,
                Alignment::Left,
            )]));
        }
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "",
            4,
            Alignment::Left,
        )]));
        if !inv.providers.is_empty() {
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment("Provider ID", 1, Alignment::Left),
                TableCell::new_with_alignment("Name", 1, Alignment::Left),
            ]));
            inv.providers.iter().for_each(|p| {
                let p = p.clone();
                table.add_row(Row::new(vec![
                    TableCell::new_with_alignment(p.id, 1, Alignment::Left),
                    TableCell::new_with_alignment(format_optional(p.name), 1, Alignment::Left),
                ]))
            });
        } else {
            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                "No providers found",
                4,
                Alignment::Left,
            )]));
        }
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "",
            4,
            Alignment::Left,
        )]));
    });

    table.render()
}

/// Helper function to transform a ClaimsList into a table string for printing
pub fn claims_table(list: Vec<HashMap<String, String>>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 2);

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        "Claims",
        2,
        Alignment::Center,
    )]));

    list.iter().for_each(|c| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Issuer", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("issuer")
                    .or_else(|| c.get("iss"))
                    .unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Subject", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("subject")
                    .or_else(|| c.get("sub"))
                    .unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Capabilities", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("capabilities")
                    .or_else(|| c.get("caps"))
                    .unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Version", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("version").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Revision", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("revision")
                    .or_else(|| c.get("rev"))
                    .unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            String::new(),
            2,
            Alignment::Center,
        )]));
    });

    table.render()
}

/// Helper function to transform a list of plugin metadata into a table string for printing
pub fn plugins_table(list: Vec<&Metadata>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Name", 1, Alignment::Left),
        TableCell::new_with_alignment("ID", 1, Alignment::Left),
        TableCell::new_with_alignment("Version", 1, Alignment::Left),
        TableCell::new_with_alignment("Author", 1, Alignment::Left),
    ]));
    list.into_iter().for_each(|metadata| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(&metadata.name, 1, Alignment::Left),
            TableCell::new_with_alignment(&metadata.id, 1, Alignment::Left),
            TableCell::new_with_alignment(&metadata.version, 1, Alignment::Left),
            TableCell::new_with_alignment(&metadata.author, 1, Alignment::Left),
        ]));
        if !metadata.description.is_empty() {
            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                format!("  └ {}", metadata.description),
                4,
                Alignment::Left,
            )]));
        }
    });

    table.render()
}
