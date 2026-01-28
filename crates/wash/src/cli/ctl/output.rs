use std::collections::HashMap;

use crate::lib::{cli::CommandOutput, plugin::subcommand::Metadata};
use serde_json::json;
use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table,
};
use wasmcloud_control_interface::{Host, HostInventory, Link};

use crate::util::format_optional;

#[must_use]
pub fn get_hosts_output(hosts: Vec<Host>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("hosts".to_string(), json!(hosts));
    CommandOutput::new(hosts_table(hosts), map)
}

#[must_use]
pub fn get_host_inventories_output(invs: Vec<HostInventory>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("inventories".to_string(), json!(invs));
    CommandOutput::new(host_inventories_table(invs), map)
}

#[must_use]
pub fn get_claims_output(claims: Vec<HashMap<String, String>>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("claims".to_string(), json!(claims));
    CommandOutput::new(claims_table(claims), map)
}

#[must_use]
pub fn links_table(mut list: Vec<Link>) -> String {
    // Sort the list based on the `source_id` field in ascending order
    list.sort_by(|a, b| a.source_id().cmp(b.source_id()));

    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![
        TableCell::new("Source ID"),
        TableCell::new("Target"),
        TableCell::new("Interface(s)"),
        TableCell::new("Name"),
    ]));

    for l in &list {
        table.add_row(Row::new(vec![
            TableCell::new(l.source_id().to_string()),
            TableCell::new(l.target().to_string()),
            TableCell::new(format!(
                "{}:{}/{}",
                l.wit_namespace(),
                l.wit_package(),
                l.interfaces().join(",")
            )),
            TableCell::new(l.name().to_string()),
        ]));
    }

    table.render()
}

/// Helper function to transform a Host list into a table string for printing
#[must_use]
pub fn hosts_table(mut hosts: Vec<Host>) -> String {
    // Sort hosts by uptime_seconds in descending order
    // hosts.sort_by(|a, b| b.uptime_seconds().cmp(&a.uptime_seconds()));
    hosts.sort_by_key(|a| std::cmp::Reverse(a.uptime_seconds()));

    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![
        TableCell::builder("Host ID").col_span(2).build(),
        TableCell::new("Friendly name"),
        TableCell::new("Uptime (seconds)"),
    ]));

    for h in &hosts {
        table.add_row(Row::new(vec![
            TableCell::builder(h.id().to_string()).col_span(2).build(),
            TableCell::new(h.friendly_name().to_string()),
            TableCell::new(format!("{}", h.uptime_seconds())),
        ]));
    }

    table.render()
}

/// Helper function to transform a `HostInventory` into a table string for printing
#[must_use]
pub fn host_inventories_table(mut invs: Vec<HostInventory>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 3);

    // Sort the host inventories alphabetically by host_id
    invs.sort_by(|a, b| a.host_id().cmp(b.host_id()));

    for inv in invs {
        table.add_row(Row::new(vec![
            TableCell::builder("Host ID").col_span(2).build(),
            TableCell::new("Friendly name"),
        ]));
        table.add_row(Row::new(vec![
            TableCell::builder(inv.host_id().to_string())
                .col_span(2)
                .build(),
            TableCell::new(inv.friendly_name().to_string()),
        ]));

        // Sort the labels alphabetically by key
        let mut sorted_labels: Vec<_> = inv.labels().iter().collect();
        sorted_labels.sort_by(|a, b| a.0.cmp(b.0));

        if !sorted_labels.is_empty() {
            table.add_row(Row::new(vec![TableCell::builder("").col_span(3).build()]));
            table.add_row(Row::new(vec![TableCell::new("Host labels")]));
            for (k, v) in sorted_labels {
                table.add_row(Row::new(vec![TableCell::new(k), TableCell::new(v)]));
            }
        } else {
            table.add_row(Row::new(vec![TableCell::builder("No labels present")
                .col_span(4)
                .alignment(Alignment::Center)
                .build()]));
        }

        table.add_row(Row::new(vec![TableCell::builder("")
            .col_span(4)
            .alignment(Alignment::Center)
            .build()]));

        // Sort the components alphabetically by name
        let mut components = inv.components().clone();
        components.sort_by(|a, b| a.name().cmp(&b.name()));

        if !components.is_empty() {
            table.add_row(Row::new(vec![
                TableCell::new("Component ID"),
                TableCell::new("Name"),
                TableCell::new("Max count"),
            ]));
            for a in &components {
                let a = a.clone();
                table.add_row(Row::new(vec![
                    TableCell::new(a.id()),
                    TableCell::new(format_optional(a.name().map(String::from))),
                    TableCell::new(a.max_instances()),
                ]));
            }
        } else {
            table.add_row(Row::new(vec![TableCell::builder("No components found")
                .col_span(4)
                .build()]));
        }

        table.add_row(Row::new(vec![TableCell::builder("").col_span(4).build()]));

        // Sort the providers alphabetically by name
        let mut providers = inv.providers().clone();
        providers.sort_by(|a, b| a.name().cmp(&b.name()));

        if !providers.is_empty() {
            table.add_row(Row::new(vec![
                TableCell::new("Provider ID"),
                TableCell::new("Name"),
            ]));
            inv.providers().iter().for_each(|p| {
                let p = p.clone();
                table.add_row(Row::new(vec![
                    TableCell::new(p.id()),
                    TableCell::new(format_optional(p.name().map(String::from))),
                ]));
            });
        } else {
            table.add_row(Row::new(vec![TableCell::builder("No providers found")
                .col_span(4)
                .build()]));
        }
        table.add_row(Row::new(vec![TableCell::builder("").col_span(4).build()]));
    }

    table.render()
}

/// Helper function to transform a `ClaimsList` into a table string for printing
#[must_use]
pub fn claims_table(list: Vec<HashMap<String, String>>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 2);

    table.add_row(Row::new(vec![TableCell::builder("Claims")
        .col_span(2)
        .alignment(Alignment::Center)
        .build()]));

    for c in &list {
        table.add_row(Row::new(vec![
            TableCell::new("Issuer"),
            TableCell::new(
                c.get("issuer")
                    .or_else(|| c.get("iss"))
                    .unwrap_or(&String::new()),
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new("Subject"),
            TableCell::new(
                c.get("subject")
                    .or_else(|| c.get("sub"))
                    .unwrap_or(&String::new()),
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new("Capabilities"),
            TableCell::new(
                c.get("capabilities")
                    .or_else(|| c.get("caps"))
                    .unwrap_or(&String::new()),
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new("Version"),
            TableCell::new(c.get("version").unwrap_or(&String::new())),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new("Revision"),
            TableCell::new(
                c.get("revision")
                    .or_else(|| c.get("rev"))
                    .unwrap_or(&String::new()),
            ),
        ]));
        table.add_row(Row::new(vec![TableCell::builder(String::new())
            .col_span(2)
            .alignment(Alignment::Center)
            .build()]));
    }

    table.render()
}

/// Helper function to transform a list of plugin metadata into a table string for printing
#[must_use]
pub fn plugins_table(list: Vec<&Metadata>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![
        TableCell::new("Name"),
        TableCell::new("ID"),
        TableCell::new("Version"),
        TableCell::new("Author"),
    ]));
    for metadata in list {
        table.add_row(Row::new(vec![
            TableCell::new(&metadata.name),
            TableCell::new(&metadata.id),
            TableCell::new(&metadata.version),
            TableCell::new(&metadata.author),
        ]));
        if !metadata.description.is_empty() {
            table.add_row(Row::new(vec![TableCell::builder(format!(
                "  └ {}",
                metadata.description
            ))
            .col_span(4)
            .build()]));
        }
    }

    table.render()
}
