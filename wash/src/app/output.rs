use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table,
};

use super::{ModelRevision, ModelSummary};

pub(crate) fn show_model_details(raw: String, vetted: String) -> String {
    format!(
        "Wrote model details to files:\nRaw: {}\nJSON: {}",
        raw, vetted
    )
}

pub(crate) fn list_revisions_table(revisions: Vec<ModelRevision>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Version", 1, Alignment::Left),
        TableCell::new_with_alignment("Created", 1, Alignment::Left),
        TableCell::new_with_alignment("Deployed?", 1, Alignment::Left),
    ]));

    revisions.iter().for_each(|r| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(r.version.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(r.created.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(r.deployed, 1, Alignment::Left),
        ]));
    });

    table.render()
}

pub(crate) fn list_models_table(models: Vec<ModelSummary>) -> String {
    let mut table = Table::new();
    crate::util::configure_table_style(&mut table);

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Name", 1, Alignment::Left),
        TableCell::new_with_alignment("Latest Version", 1, Alignment::Left),
        TableCell::new_with_alignment("Description", 1, Alignment::Left),
        TableCell::new_with_alignment("Deploy Status", 1, Alignment::Right),
    ]));
    models.iter().for_each(|m| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(m.name.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(m.version.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(m.description.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(m.deployment_status.clone(), 1, Alignment::Right),
        ]))
    });

    table.render()
}
