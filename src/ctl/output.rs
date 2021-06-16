extern crate wasmcloud_control_interface;
use crate::util::{format_ellipsis, format_optional, format_output, OutputKind, WASH_CMD_INFO};
use log::debug;
use serde_json::json;
use term_table::{row::Row, table_cell::*, Table};
use wasmcloud_control_interface::*;

// Helper output functions, used to ensure consistent output between ctl & standalone commands

pub(crate) fn call_output(error: Option<String>, msg: Vec<u8>, output_kind: &OutputKind) -> String {
    match error {
        Some(e) => format_output(
            format!("\nError invoking actor: {}", e),
            json!({ "error": e }),
            &output_kind,
        ),
        None => {
            //TODO(issue #32): String::from_utf8_lossy should be decoder only if one is not available
            let call_response = String::from_utf8_lossy(&msg);
            format_output(
                format!("\nCall response (raw): {}", call_response),
                json!({ "response": call_response }),
                &output_kind,
            )
        }
    }
}
pub(crate) fn get_hosts_output(hosts: Vec<Host>, output_kind: &OutputKind) -> String {
    debug!(target: WASH_CMD_INFO, "Hosts:{:?}", hosts);
    match *output_kind {
        OutputKind::Text { max_width } => hosts_table(hosts, max_width),
        OutputKind::Json => format!("{}", json!({ "hosts": hosts })),
    }
}
pub(crate) fn get_host_inventory_output(inv: HostInventory, output_kind: &OutputKind) -> String {
    debug!(target: WASH_CMD_INFO, "Inventory:{:?}", inv);
    match *output_kind {
        OutputKind::Text { max_width } => host_inventory_table(inv, max_width),
        OutputKind::Json => format!("{}", json!({ "inventory": inv })),
    }
}
pub(crate) fn get_claims_output(claims: ClaimsList, output_kind: &OutputKind) -> String {
    debug!(target: WASH_CMD_INFO, "Claims:{:?}", claims);
    match *output_kind {
        OutputKind::Text { max_width } => claims_table(claims, max_width),
        OutputKind::Json => format!("{}", json!({ "claims": claims })),
    }
}
pub(crate) fn link_output(
    actor_id: &str,
    provider_id: &str,
    failure: Option<String>,
    output_kind: &OutputKind,
) -> String {
    debug!(
        target: WASH_CMD_INFO,
        "Publishing link between {} and {}", actor_id, provider_id
    );
    match failure {
        None => format_output(
            format!(
                "\nAdvertised link ({}) <-> ({}) successfully",
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
pub(crate) fn start_actor_output(
    actor_ref: &str,
    host_id: &str,
    failure: Option<String>,
    output_kind: &OutputKind,
) -> String {
    debug!(
        target: WASH_CMD_INFO,
        "Sending request to start actor {}", actor_ref
    );
    match failure {
        None => format_output(
            format!("\nActor starting on host {}", host_id),
            json!({ "actor_ref": actor_ref, "host_id": host_id }),
            &output_kind,
        ),
        Some(f) => format_output(
            format!("\nError starting actor: {}", f),
            json!({ "error": f }),
            &output_kind,
        ),
    }
}
pub(crate) fn start_provider_output(
    provider_ref: &str,
    host_id: &str,
    failure: Option<String>,
    output_kind: &OutputKind,
) -> String {
    debug!(
        target: WASH_CMD_INFO,
        "Sending request to start provider {}", provider_ref
    );
    match failure {
        None => format_output(
            format!("\nProvider starting on host {}", host_id),
            json!({ "provider_ref": provider_ref, "host_id": host_id}),
            output_kind,
        ),
        Some(e) => format_output(
            format!("\nError starting provider: {}", e),
            json!({ "error": e }),
            output_kind,
        ),
    }
}
pub(crate) fn stop_actor_output(
    actor_ref: &str,
    failure: Option<String>,
    output_kind: &OutputKind,
) -> String {
    match failure {
        Some(f) => format_output(
            format!("\nError stopping actor: {}", f),
            json!({ "error": f }),
            &output_kind,
        ),
        None => format_output(
            format!("\nStopping actor: {}", actor_ref),
            json!({ "actor_ref": actor_ref }),
            &output_kind,
        ),
    }
}
pub(crate) fn stop_provider_output(
    provider_ref: &str,
    failure: Option<String>,
    output_kind: &OutputKind,
) -> String {
    match failure {
        Some(f) => format_output(
            format!("\nError stopping provider: {}", f),
            json!({ "error": f }),
            output_kind,
        ),
        None => format_output(
            format!("\nStopping provider: {}", provider_ref),
            json!({ "provider_ref": provider_ref }),
            output_kind,
        ),
    }
}
pub(crate) fn update_actor_output(
    actor_id: &str,
    new_actor_ref: &str,
    error: Option<String>,
    output_kind: &OutputKind,
) -> String {
    if let Some(e) = error {
        format_output(
            format!("\nError updating actor: {}", e),
            json!({ "error": e }),
            output_kind,
        )
    } else {
        format_output(
            format!("\nActor {} updated to {}", actor_id, new_actor_ref),
            json!({ "accepted": error.is_none() }),
            output_kind,
        )
    }
}

/// Helper function to print a Host list to stdout as a table
pub(crate) fn hosts_table(hosts: Vec<Host>, max_width: usize) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 2, max_width);

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

/// Helper function to print a HostInventory to stdout as a table
pub(crate) fn host_inventory_table(inv: HostInventory, max_width: usize) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4, max_width);

    // TableCells have 1 char padding left and right. See [TableCell::pad_content].
    let content_padding_width = 2;
    let max_id_width = crate::util::get_max_column_width(&table, 0) - content_padding_width;

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

    if !inv.actors.is_empty() {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "",
            4,
            Alignment::Center,
        )]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Actor ID", 1, Alignment::Left),
            TableCell::new_with_alignment("Name", 1, Alignment::Left),
            TableCell::new_with_alignment("Image Reference", 2, Alignment::Left),
        ]));
        inv.actors.iter().for_each(|a| {
            let a = a.clone();
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment(
                    format_ellipsis(a.id, max_id_width),
                    1,
                    Alignment::Left,
                ),
                TableCell::new_with_alignment(format_optional(a.name), 1, Alignment::Left),
                TableCell::new_with_alignment(format_optional(a.image_ref), 2, Alignment::Left),
            ]))
        });
    } else {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "No actors found",
            4,
            Alignment::Center,
        )]));
    }

    if !inv.providers.is_empty() {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "",
            4,
            Alignment::Center,
        )]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Provider ID", 1, Alignment::Left),
            TableCell::new_with_alignment("Name", 1, Alignment::Left),
            TableCell::new_with_alignment("Link Name", 1, Alignment::Left),
            TableCell::new_with_alignment("Image Reference", 1, Alignment::Left),
        ]));
        inv.providers.iter().for_each(|p| {
            let p = p.clone();
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment(
                    format_ellipsis(p.id, max_id_width),
                    1,
                    Alignment::Left,
                ),
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

/// Helper function to print a ClaimsList to stdout as a table
pub(crate) fn claims_table(list: ClaimsList, max_width: usize) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 2, max_width);

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        "Claims",
        2,
        Alignment::Center,
    )]));

    list.claims.iter().for_each(|c| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Issuer", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("iss").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Subject", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("sub").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Capabilities", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("caps").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Version", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("version").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Revision", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("rev").unwrap_or(&"".to_string()),
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
