extern crate wasmcloud_control_interface;
use crate::util::{format_optional, format_output, OutputKind};
use serde_json::json;
use term_table::{row::Row, table_cell::*, Table};
use wasmcloud_control_interface::*;

use super::id::{ModuleId, ServiceId};

pub(crate) fn get_hosts_output(hosts: Vec<Host>, output_kind: &OutputKind) -> String {
    match *output_kind {
        OutputKind::Text => hosts_table(hosts),
        OutputKind::Json => format!("{}", json!({ "hosts": hosts })),
    }
}

pub(crate) fn get_host_inventory_output(inv: HostInventory, output_kind: &OutputKind) -> String {
    match *output_kind {
        OutputKind::Text => host_inventory_table(inv),
        OutputKind::Json => format!("{}", json!({ "inventory": inv })),
    }
}

pub(crate) fn get_claims_output(claims: GetClaimsResponse, output_kind: &OutputKind) -> String {
    match *output_kind {
        OutputKind::Text => claims_table(claims),
        OutputKind::Json => format!("{}", json!({ "claims": claims })),
    }
}

pub(crate) fn link_del_output(
    actor_id: &ModuleId,
    contract_id: &str,
    link_name: &str,
    failure: Option<String>,
    output_kind: &OutputKind,
) -> String {
    match failure {
        None => format_output(
            format!(
                "\nDeleted link for {} on {} ({}) successfully",
                actor_id, contract_id, link_name
            ),
            json!({"actor_id": actor_id, "contract_id": contract_id, "link_name": link_name, "result": "published"}),
            output_kind,
        ),
        Some(f) => format_output(
            format!("\nError deleting link: {}", f),
            json!({ "error": f }),
            output_kind,
        ),
    }
}

pub(crate) fn link_put_output(
    actor_id: &ModuleId,
    provider_id: &ServiceId,
    failure: Option<String>,
    output_kind: &OutputKind,
) -> String {
    match failure {
        None => format_output(
            format!(
                "\nPublished link ({}) <-> ({}) successfully",
                actor_id, provider_id
            ),
            json!({"actor_id": actor_id, "provider_id": provider_id, "result": "published"}),
            output_kind,
        ),
        Some(f) => format_output(
            format!("\nError advertising link: {}", f),
            json!({ "error": f }),
            output_kind,
        ),
    }
}

pub(crate) fn link_query_output(list: LinkDefinitionList, output_kind: &OutputKind) -> String {
    match *output_kind {
        OutputKind::Text => links_table(list),
        OutputKind::Json => format!("{}", json!({ "links": list.links })),
    }
}

pub(crate) fn apply_manifest_output(results: Vec<String>, output_kind: &OutputKind) -> String {
    format_output(
        format!("\nManifest application results:\n{}", results.join("\n")),
        json!({ "results": results }),
        output_kind,
    )
}

pub(crate) fn ctl_operation_output(
    accepted: bool,
    success: &str,
    error: &str,
    output_kind: &OutputKind,
) -> String {
    if accepted {
        format_output(
            format!("\n{}", success),
            json!({ "accepted": accepted, "error": ""}),
            output_kind,
        )
    } else {
        format_output(
            format!("\n{}", error),
            json!({ "accepted": accepted, "error": error}),
            output_kind,
        )
    }
}

/// Helper function to transform a LinkDefinitionList into a table string for printing
pub(crate) fn links_table(list: LinkDefinitionList) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Actor ID", 1, Alignment::Left),
        TableCell::new_with_alignment("Provider ID", 1, Alignment::Left),
        TableCell::new_with_alignment("Contract ID", 1, Alignment::Left),
        TableCell::new_with_alignment("Link Name", 1, Alignment::Left),
    ]));

    list.links.iter().for_each(|l| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(l.actor_id.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(l.provider_id.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(l.contract_id.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(l.link_name.clone(), 1, Alignment::Left),
        ]))
    });

    table.render()
}

/// Helper function to transform a Host list into a table string for printing
pub(crate) fn hosts_table(hosts: Vec<Host>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Host ID", 1, Alignment::Left),
        TableCell::new_with_alignment("Uptime (seconds)", 1, Alignment::Left),
    ]));
    hosts.iter().for_each(|h| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(h.id.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(format!("{}", h.uptime_seconds), 1, Alignment::Left),
        ]))
    });

    table.render()
}

/// Helper function to transform a HostInventory into a table string for printing
pub(crate) fn host_inventory_table(inv: HostInventory) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table);

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        format!("Host Inventory ({})", inv.host_id),
        4,
        Alignment::Center,
    )]));

    if !inv.labels.is_empty() {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "",
            4,
            Alignment::Center,
        )]));
        inv.labels.iter().for_each(|(k, v)| {
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment(k, 2, Alignment::Left),
                TableCell::new_with_alignment(v, 2, Alignment::Left),
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
    if !inv.actors.is_empty() {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Actor ID", 1, Alignment::Left),
            TableCell::new_with_alignment("Name", 1, Alignment::Left),
            TableCell::new_with_alignment("Image Reference", 2, Alignment::Left),
        ]));
        inv.actors.iter().for_each(|a| {
            let a = a.clone();
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment(a.id, 1, Alignment::Left),
                TableCell::new_with_alignment(format_optional(a.name), 1, Alignment::Left),
                TableCell::new_with_alignment(format_optional(a.image_ref), 2, Alignment::Left),
            ]))
        });
    } else {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "No actors found",
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
            TableCell::new_with_alignment("Link Name", 1, Alignment::Left),
            TableCell::new_with_alignment("Image Reference", 1, Alignment::Left),
        ]));
        inv.providers.iter().for_each(|p| {
            let p = p.clone();
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment(p.id, 1, Alignment::Left),
                TableCell::new_with_alignment(format_optional(p.name), 1, Alignment::Left),
                TableCell::new_with_alignment(p.link_name, 1, Alignment::Left),
                TableCell::new_with_alignment(format_optional(p.image_ref), 1, Alignment::Left),
            ]))
        });
    } else {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "No providers found",
            4,
            Alignment::Left,
        )]));
    }

    table.render()
}

/// Helper function to transform a ClaimsList into a table string for printing
pub(crate) fn claims_table(list: GetClaimsResponse) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table);

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        "Claims",
        2,
        Alignment::Center,
    )]));

    list.claims.iter().for_each(|c| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Issuer", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("iss").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Subject", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("sub").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Capabilities", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.get("caps").unwrap_or(&"".to_string()),
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
                c.get("rev").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            format!(""),
            2,
            Alignment::Center,
        )]));
    });

    table.render()
}
