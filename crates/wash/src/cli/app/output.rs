use term_table::{row::Row, table_cell::TableCell, Table};
use wadm_types::api::{Status, VersionInfo};

use super::ModelSummary;

pub fn list_revisions_table(revisions: Vec<VersionInfo>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 2);

    table.add_row(Row::new(vec![
        TableCell::new("Version"),
        TableCell::new("Deployed"),
    ]));

    for r in &revisions {
        table.add_row(Row::new(vec![
            TableCell::new(r.version.clone()),
            TableCell::new(r.deployed),
        ]));
    }

    table.render()
}

pub fn list_models_table(models: Vec<ModelSummary>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 3);
    table.add_row(Row::new(vec![
        TableCell::new("Name"),
        TableCell::new("Deployed Version"),
        TableCell::new("Status"),
    ]));
    for m in &models {
        table.add_row(Row::new(vec![
            TableCell::new(m.name.clone()),
            TableCell::new(
                m.deployed_version
                    .clone()
                    .unwrap_or_else(|| "N/A".to_string()),
            ),
            TableCell::new(format!("{:?}", m.detailed_status)),
        ]));

        if let Some(description) = m.description.as_ref() {
            table.add_row(Row::new(vec![TableCell::builder(format!(
                "  └ {description}"
            ))
            .col_span(3)
            .build()]));
        }
    }

    table.render()
}

/// Generate a table for displaying the status of a model
///
/// This table contains a lot of information, and some potentially very long strings
/// with status messages. Because of this, there's some manual formatting of strings
/// after the table
pub fn status_table(model_name: String, status: Status) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table, 4);

    table.add_row(Row::new(vec![TableCell::builder("").col_span(4).build()]));

    table.add_row(Row::new(vec![
        TableCell::builder("Name").col_span(2).build(),
        TableCell::new("Kind"),
        TableCell::new("Status"),
    ]));

    // To better display information in the table, what we want to do here is replace
    // the generated IDs in a manifest with just the component name.
    //
    // For example, turning "rust_hello_world-http_component" into "http_component"
    // by removing the model name and the "-" character.
    let mut model_name_replacer = model_name.replace('-', "_");
    model_name_replacer.push('-');
    status.scalers.iter().for_each(|s| {
        let status = if !s.info.message.is_empty() {
            format!("{:?} (*)", s.info.status_type)
        } else {
            format!("{:?}", s.info.status_type)
        };
        table.add_row(Row::new(vec![
            TableCell::builder(s.name.replace(&model_name_replacer, ""))
                .col_span(2)
                .build(),
            TableCell::new(&s.kind),
            TableCell::new(status),
        ]));
    });

    if status.scalers.iter().any(|s| !s.info.message.is_empty()) {
        table.add_row(Row::new(vec![TableCell::builder("").col_span(4).build()]));
        table.add_row(Row::new(vec![TableCell::builder("Status Messages")
            .col_span(4)
            .build()]));
    }

    let mut table_output = table.render();

    status.scalers.iter().for_each(|s| {
        if !s.info.message.is_empty() {
            table_output.push_str(&format!(
                "  {}\n    └ {}\n\n",
                &s.name.replace(&model_name_replacer, ""),
                s.info.message
            ));
        }
    });

    // Prepend the application name, version, and status type to the table output
    #[allow(deprecated)]
    let version = status.version;
    format!(
        "{}@{} - {:?}{}",
        &model_name, version, status.info.status_type, table_output
    )
}
