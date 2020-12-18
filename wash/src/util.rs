use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::str::FromStr;
use structopt::StructOpt;

pub(crate) type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error>>;

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

#[derive(StructOpt, Debug, Copy, Clone, Serialize, Deserialize)]
pub(crate) enum OutputKind {
    Text,
    JSON,
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

impl PartialEq for OutputKind {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (OutputKind::JSON, OutputKind::JSON) => true,
            (OutputKind::Text, OutputKind::Text) => true,
            _ => false,
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
            "{}",
            "error parsing output type, see help for the list of accepted outputs"
        )
    }
}

/// Returns string output for provided output kind
pub(crate) fn format_output(
    text: String,
    json: serde_json::Value,
    output_kind: &OutputKind,
) -> String {
    match output_kind {
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
