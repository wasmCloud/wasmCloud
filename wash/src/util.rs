use log::info;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::str::FromStr;
use structopt::StructOpt;
use term_table::{Table, TableStyle};

pub(crate) type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error>>;

/// Environment variable to show when user is in REPL mode
pub(crate) static REPL_MODE: OnceCell<String> = OnceCell::new();

pub(crate) const WASH_LOG_INFO: &str = "WASH_LOG";
pub(crate) const WASH_CMD_INFO: &str = "WASH_CMD";

thread_local! {
    /// Currently available output width can change when the user resizes their terminal window.
    static MAX_TEXT_OUTPUT_WIDTH: Cell<usize> = Cell::new(0);
}

#[derive(StructOpt, Debug, Copy, Clone, Deserialize, Serialize)]
pub(crate) struct Output {
    #[structopt(
        short = "o",
        long = "output",
        default_value = "text",
        help = "Specify output format (text, json or wide)"
    )]
    pub(crate) kind: OutputKind,
}

/// Used for displaying human-readable output vs JSON format
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum OutputKind {
    Text { max_width: usize },
    Json,
}

/// Used to supress `println!` macro calls in the REPL
#[derive(StructOpt, Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum OutputDestination {
    Cli,
    Repl,
}

impl Default for Output {
    fn default() -> Self {
        Output {
            kind: OutputKind::Text {
                max_width: get_max_text_output_width(),
            },
        }
    }
}

impl FromStr for OutputKind {
    type Err = OutputParseErr;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "json" => Ok(OutputKind::Json),
            "text" => Ok(OutputKind::Text {
                max_width: get_max_text_output_width(),
            }),
            "wide" => Ok(OutputKind::Text {
                max_width: usize::MAX,
            }),
            _ => Err(OutputParseErr),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputParseErr;

impl Error for OutputParseErr {}

impl fmt::Display for OutputParseErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "error parsing output type, see help for the list of accepted outputs"
        )
    }
}

pub(crate) fn set_max_text_output_width(width: usize) {
    MAX_TEXT_OUTPUT_WIDTH.with(|output_width| {
        output_width.set(width);
    });
}

pub(crate) fn get_max_text_output_width() -> usize {
    MAX_TEXT_OUTPUT_WIDTH.with(Cell::get)
}

/// Returns string output for provided output kind
pub(crate) fn format_output(
    text: String,
    json: serde_json::Value,
    output_kind: &OutputKind,
) -> String {
    match output_kind {
        OutputKind::Text { .. } => text,
        OutputKind::Json => format!("{}", json),
    }
}

pub(crate) fn format_ellipsis(id: String, max_width: usize) -> String {
    if id.len() > max_width {
        let ellipsis = "...";
        id.chars()
            .take(max_width - ellipsis.len())
            .collect::<String>()
            + ellipsis
    } else {
        id
    }
}

pub(crate) fn format_optional(value: Option<String>) -> String {
    value.unwrap_or_else(|| "N/A".into())
}

/// Returns value from an argument that may be a file path or the value itself
pub(crate) fn extract_arg_value(arg: &str) -> Result<String> {
    match File::open(arg) {
        Ok(mut f) => {
            let mut value = String::new();
            f.read_to_string(&mut value)?;
            Ok(value)
        }
        Err(_) => Ok(arg.to_string()),
    }
}

/// Converts error from Send + Sync error to standard error
pub(crate) fn convert_error(
    e: Box<dyn ::std::error::Error + Send + Sync>,
) -> Box<dyn ::std::error::Error> {
    Box::<dyn std::error::Error>::from(format!("{}", e))
}

/// Transforms a list of labels in the form of (label=value) to a hashmap
pub(crate) fn labels_vec_to_hashmap(constraints: Vec<String>) -> Result<HashMap<String, String>> {
    let mut hm: HashMap<String, String> = HashMap::new();
    for constraint in constraints {
        let key_value = constraint.split('=').collect::<Vec<_>>();
        if key_value.len() < 2 {
            return Err(
                "Constraints were not properly formatted. Ensure they are formatted as label=value"
                    .into(),
            );
        }
        hm.insert(key_value[0].to_string(), key_value[1].to_string()); // [0] key, [1] value
    }
    Ok(hm)
}

/// Transform a json str (e.g. "{"hello": "world"}") into msgpack bytes
pub(crate) fn json_str_to_msgpack_bytes(payload: Vec<String>) -> Result<Vec<u8>> {
    let json: serde_json::value::Value = serde_json::from_str(&payload.join(""))?;
    let payload = serdeconv::to_msgpack_vec(&json)?;
    Ok(payload)
}

/// Helper function to either display input to stdout or log the output in the REPL
pub(crate) fn print_or_log(output: String) {
    match output_destination() {
        OutputDestination::Repl => info!(target: WASH_LOG_INFO, "{}", output),
        OutputDestination::Cli => println!("{}", output),
    }
}

/// Helper function to retrieve REPL_MODE environment variable to determine output destination
pub(crate) fn output_destination() -> OutputDestination {
    // REPL_MODE is Some("true") when in REPL, otherwise CLI
    match REPL_MODE.get() {
        Some(_) => OutputDestination::Repl,
        None => OutputDestination::Cli,
    }
}

pub(crate) fn configure_table_style(table: &mut Table<'_>, columns: usize, max_table_width: usize) {
    table.max_column_width = if max_table_width > 0 && columns > 0 {
        let borders = 1 + columns;
        (max_table_width - borders) / columns
    } else {
        usize::MAX
    };
    table.style = empty_table_style();
    table.separate_rows = false;
}

pub(crate) fn get_max_column_width(table: &Table<'_>, column_index: usize) -> usize {
    *table
        .max_column_widths
        .get(&column_index)
        .unwrap_or(&table.max_column_width)
}

fn empty_table_style() -> TableStyle {
    TableStyle {
        top_left_corner: ' ',
        top_right_corner: ' ',
        bottom_left_corner: ' ',
        bottom_right_corner: ' ',
        outer_left_vertical: ' ',
        outer_right_vertical: ' ',
        outer_bottom_horizontal: ' ',
        outer_top_horizontal: ' ',
        intersection: ' ',
        vertical: ' ',
        horizontal: ' ',
    }
}

#[cfg(test)]
mod test {
    use super::{configure_table_style, format_ellipsis};
    use term_table::{row::Row, table_cell::TableCell, Table};

    #[test]
    fn format_ellipsis_truncates_to_max_width() {
        assert_eq!("hello w...", &format_ellipsis("hello world".into(), 10));
    }

    #[test]
    fn format_ellipsis_no_ellipsis_necessary() {
        assert_eq!("hello world", &format_ellipsis("hello world".into(), 11));
    }

    #[test]
    fn max_table_width_one_column() {
        let mut table = Table::new();
        configure_table_style(&mut table, 1, 10);
        table.add_row(Row::new(vec![TableCell::new("x".repeat(10))]));
        let result = table.render();
        let max_line_width = result.lines().map(|line| line.len()).max().unwrap();

        assert_eq!(10, max_line_width);
    }

    #[test]
    fn max_table_width_two_columns() {
        let mut table = Table::new();
        configure_table_style(&mut table, 2, 10);
        table.add_row(Row::new(vec![
            TableCell::new("x".repeat(5)),
            TableCell::new("y".repeat(5)),
        ]));
        let result = table.render();
        let max_line_width = result.lines().map(|line| line.len()).max().unwrap();

        assert_eq!(9, max_line_width);
    }

    #[test]
    fn max_table_width_two_columns_spanned() {
        let mut table = Table::new();
        configure_table_style(&mut table, 2, 10);
        table.add_row(Row::new(vec![TableCell::new_with_col_span(
            "x".repeat(10),
            2,
        )]));
        let result = table.render();
        let max_line_width = result.lines().map(|line| line.len()).max().unwrap();

        assert_eq!(9, max_line_width);
    }
}
