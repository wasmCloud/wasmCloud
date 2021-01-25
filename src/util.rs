use log::info;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::str::FromStr;
use structopt::StructOpt;

pub(crate) type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error>>;

/// Environment variable to show when user is in REPL mode
pub(crate) static REPL_MODE: OnceCell<String> = OnceCell::new();

pub(crate) const WASH_LOG_INFO: &str = "WASH_LOG";
pub(crate) const WASH_CMD_INFO: &str = "WASH_CMD";

#[derive(StructOpt, Debug, Copy, Clone, Deserialize, Serialize)]
pub(crate) struct Output {
    #[structopt(
        short = "o",
        long = "output",
        default_value = "text",
        help = "Specify output format (text or json)"
    )]
    pub(crate) kind: OutputKind,
}

/// Used for displaying human-readable output vs JSON format
#[derive(StructOpt, Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum OutputKind {
    Text,
    JSON,
}

/// Used to supress `println!` macro calls in the REPL
#[derive(StructOpt, Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum OutputDestination {
    CLI,
    REPL,
}

impl Default for Output {
    fn default() -> Self {
        Output {
            kind: OutputKind::Text,
        }
    }
}

impl FromStr for OutputKind {
    type Err = OutputParseErr;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "json" => Ok(OutputKind::JSON),
            "text" => Ok(OutputKind::Text),
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

/// Returns string output for provided output kind
pub(crate) fn format_output(text: String, json: serde_json::Value, output: &Output) -> String {
    match output.kind {
        OutputKind::Text => text,
        OutputKind::JSON => format!("{}", json),
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

/// Transform a json str (e.g. "{"hello": "world"}") and transform it into msgpack bytes
pub(crate) fn json_str_to_msgpack_bytes(payload: Vec<String>) -> Result<Vec<u8>> {
    let json: serde_json::value::Value = serde_json::from_str(&payload.join(""))?;
    let payload = serdeconv::to_msgpack_vec(&json)?;
    Ok(payload)
}

/// Helper function to either display input to stdout or log the output in the REPL
pub(crate) fn print_or_log(output: String) {
    match output_destination() {
        OutputDestination::REPL => info!(target: WASH_LOG_INFO, "{}", output),
        OutputDestination::CLI => println!("{}", output),
    }
}

/// Helper function to retrieve REPL_MODE environment variable to determine output destination
pub(crate) fn output_destination() -> OutputDestination {
    // REPL_MODE is Some("true") when in REPL, otherwise CLI
    match REPL_MODE.get() {
        Some(_) => OutputDestination::REPL,
        None => OutputDestination::CLI,
    }
}
