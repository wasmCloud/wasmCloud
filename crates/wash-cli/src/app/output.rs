use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table,
};
use wadm_types::api::{Status, VersionInfo};

use super::ModelSummary;

pub fn list_revisions_table(revisions: Vec<VersionInfo>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Version", 1, Alignment::Left),
        TableCell::new_with_alignment("Deployed", 1, Alignment::Left),
    ]));

    revisions.iter().for_each(|r| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(r.version.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(r.deployed, 1, Alignment::Left),
        ]));
    });

    table.render()
}

pub fn list_models_table(models: Vec<ModelSummary>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Name", 1, Alignment::Left),
        TableCell::new_with_alignment("Latest Version", 1, Alignment::Left),
        TableCell::new_with_alignment("Deployed Version", 1, Alignment::Left),
        TableCell::new_with_alignment("Deploy Status", 1, Alignment::Right),
        TableCell::new_with_alignment("Description", 1, Alignment::Left),
    ]));
    models.iter().for_each(|m| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(m.name.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(m.version.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(
                m.deployed_version
                    .clone()
                    .unwrap_or_else(|| "N/A".to_string()),
                1,
                Alignment::Left,
            ),
            TableCell::new_with_alignment(format!("{:?}", m.status), 1, Alignment::Right),
            TableCell::new_with_alignment(
                m.description.clone().unwrap_or_else(|| "N/A".to_string()),
                1,
                Alignment::Left,
            ),
        ]))
    });

    table.render()
}

pub fn status_table(model_name: String, status: Status) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Name", 1, Alignment::Left),
        TableCell::new_with_alignment("Deployed Version", 1, Alignment::Left),
        TableCell::new_with_alignment("Deploy Status", 1, Alignment::Left),
        TableCell::new_with_alignment("Status Message", 1, Alignment::Left),
    ]));

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment(model_name, 1, Alignment::Left),
        TableCell::new_with_alignment(status.version, 1, Alignment::Left),
        TableCell::new_with_alignment(format!("{:?}", status.info.status_type), 1, Alignment::Left),
        TableCell::new_with_alignment(status.info.message, 1, Alignment::Left),
    ]));

    table.render()
}
