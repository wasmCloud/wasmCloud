use std::collections::HashMap;

use serde_json::json;
use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table,
};
use crate::lib::{cli::CommandOutput, plugin::subcommand::Metadata};
use wasmcloud_control_interface::{Host, HostInventory, Link};

use crate::util::format_optional;

#[must_use] pub fn get_hosts_output(hosts: Vec<Host>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("hosts".to_string(), json!(hosts));
    CommandOutput::new(hosts_table(hosts), map)
}

#[must_use] pub fn get_host_inventories_output(invs: Vec<HostInventory>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("inventories".to_string(), json!(invs));
    CommandOutput::new(host_inventories_table(invs), map)
}

#[must_use] pub fn get_claims_output(claims: Vec<HashMap<String, String>>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("claims".to_string(), json!(claims));
    CommandOutput::new(claims_table(claims), map)
}

#[must_use] pub fn links_table(mut list: Vec<Link>) -> String {
    // Sort the list based on the `source_id` field in ascending order
    list.sort_by(|a, b| a.source_id().cmp(b.source_id()));

    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Source ID", 1, Alignment::Left),
        TableCell::new_with_alignment("Target", 1, Alignment::Left),
        TableCell::new_with_alignment("Interface(s)", 1, Alignment::Left),
        TableCell::new_with_alignment("Name", 1, Alignment::Left),
    ]));

    for l in &list {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(l.source_id().to_string(), 1, Alignment::Left),
            TableCell::new_with_alignment(l.target().to_string(), 1, Alignment::Left),
            TableCell::new_with_alignment(
                format!(
                    "{}:{}/{}",
                    l.wit_namespace(),
                    l.wit_package(),
                    l.interfaces().join(",")
                ),
                1,
                Alignment::Left,
            ),
            TableCell::new_with_alignment(l.name().to_string(), 1, Alignment::Left),
        ]));
    }

    table.render()
}

/// Helper function to transform a Host list into a table string for printing
#[must_use] pub fn hosts_table(mut hosts: Vec<Host>) -> String {
    // Sort hosts by uptime_seconds in descending order
    // hosts.sort_by(|a, b| b.uptime_seconds().cmp(&a.uptime_seconds()));
    hosts.sort_by_key(|a| std::cmp::Reverse(a.uptime_seconds()));

    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Host ID", 2, Alignment::Left),
        TableCell::new_with_alignment("Friendly name", 1, Alignment::Left),
        TableCell::new_with_alignment("Uptime (seconds)", 1, Alignment::Left),
    ]));

    for h in &hosts {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(h.id().to_string(), 2, Alignment::Left),
            TableCell::new_with_alignment(h.friendly_name().to_string(), 1, Alignment::Left),
            TableCell::new_with_alignment(format!("{}", h.uptime_seconds()), 1, Alignment::Left),
        ]));
    }

    table.render()
}

/// Helper function to transform a `HostInventory` into a table string for printing
#[must_use] pub fn host_inventories_table(mut invs: Vec<HostInventory>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 3);

    // Sort the host inventories alphabetically by host_id
    invs.sort_by(|a, b| a.host_id().cmp(b.host_id()));

    for inv in invs {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Host ID", 2, Alignment::Left),
            TableCell::new_with_alignment("Friendly name", 1, Alignment::Left),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(inv.host_id().to_string(), 2, Alignment::Left),
            TableCell::new_with_alignment(inv.friendly_name().to_string(), 1, Alignment::Left),
        ]));

        // Sort the labels alphabetically by key
        let mut sorted_labels: Vec<_> = inv.labels().iter().collect();
        sorted_labels.sort_by(|a, b| a.0.cmp(b.0));

        if !sorted_labels.is_empty() {
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
            for (k, v) in sorted_labels {
                table.add_row(Row::new(vec![
                    TableCell::new_with_alignment(k, 1, Alignment::Left),
                    TableCell::new_with_alignment(v, 1, Alignment::Left),
                ]));
            }
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

        // Sort the components alphabetically by name
        let mut components = inv.components().clone();
        components.sort_by(|a, b| a.name().cmp(&b.name()));

        if !components.is_empty() {
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment("Component ID", 1, Alignment::Left),
                TableCell::new_with_alignment("Name", 1, Alignment::Left),
                TableCell::new_with_alignment("Max count", 1, Alignment::Left),
            ]));
            for a in &components {
                let a = a.clone();
                table.add_row(Row::new(vec![
                    TableCell::new_with_alignment(a.id(), 1, Alignment::Left),
                    TableCell::new_with_alignment(
                        format_optional(a.name().map(String::from)),
                        1,
                        Alignment::Left,
                    ),
                    TableCell::new_with_alignment(a.max_instances(), 1, Alignment::Left),
                ]));
            }
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

        // Sort the providers alphabetically by name
        let mut providers = inv.providers().clone();
        providers.sort_by(|a, b| a.name().cmp(&b.name()));

        if !providers.is_empty() {
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment("Provider ID", 1, Alignment::Left),
                TableCell::new_with_alignment("Name", 1, Alignment::Left),
            ]));
            inv.providers().iter().for_each(|p| {
                let p = p.clone();
                table.add_row(Row::new(vec![
                    TableCell::new_with_alignment(p.id(), 1, Alignment::Left),
                    TableCell::new_with_alignment(
                        format_optional(p.name().map(String::from)),
                        1,
                        Alignment::Left,
                    ),
                ]));
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
    }

    table.render()
}

/// Helper function to transform a `ClaimsList` into a table string for printing
#[must_use] pub fn claims_table(list: Vec<HashMap<String, String>>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 2);

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        "Claims",
        2,
        Alignment::Center,
    )]));

    for c in &list {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Issuer", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("issuer")
                    .or_else(|| c.get("iss"))
                    .unwrap_or(&String::new()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Subject", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("subject")
                    .or_else(|| c.get("sub"))
                    .unwrap_or(&String::new()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Capabilities", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("capabilities")
                    .or_else(|| c.get("caps"))
                    .unwrap_or(&String::new()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Version", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("version").unwrap_or(&String::new()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Revision", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("revision")
                    .or_else(|| c.get("rev"))
                    .unwrap_or(&String::new()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            String::new(),
            2,
            Alignment::Center,
        )]));
    }

    table.render()
}

/// Helper function to transform a list of plugin metadata into a table string for printing
#[must_use] pub fn plugins_table(list: Vec<&Metadata>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Name", 1, Alignment::Left),
        TableCell::new_with_alignment("ID", 1, Alignment::Left),
        TableCell::new_with_alignment("Version", 1, Alignment::Left),
        TableCell::new_with_alignment("Author", 1, Alignment::Left),
    ]));
    for metadata in list {
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
    }

    table.render()
}
