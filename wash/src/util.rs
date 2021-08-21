use nats::asynk::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::str::FromStr;
use structopt::StructOpt;
use term_table::{Table, TableStyle};

pub(crate) type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error>>;

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
    Text,
    Json,
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
            "json" => Ok(OutputKind::Json),
            "text" => Ok(OutputKind::Text),
            "wide" => Ok(OutputKind::Text),
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
pub(crate) fn format_output(
    text: String,
    json: serde_json::Value,
    output_kind: &OutputKind,
) -> String {
    match output_kind {
        OutputKind::Text => text,
        OutputKind::Json => serde_json::to_string(&json).unwrap(),
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
    Box::<dyn std::error::Error>::from(e.to_string())
}

/// Converts error from RpcError
pub(crate) fn convert_rpc_error(e: wasmbus_rpc::RpcError) -> Box<dyn ::std::error::Error> {
    Box::<dyn std::error::Error>::from(e.to_string())
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

/// Transform a json string (e.g. "{"hello": "world"}") into msgpack bytes
pub(crate) fn json_str_to_msgpack_bytes(payload: &str) -> Result<Vec<u8>> {
    let json = serde_json::from_str::<serde_json::Value>(payload)?;
    let payload = wasmbus_rpc::serialize(&json)?;
    Ok(payload)
}

use once_cell::sync::OnceCell;
static BIN_STR: OnceCell<char> = OnceCell::new();

fn msgpack_to_json(mval: rmpv::Value) -> serde_json::Value {
    use rmpv::Value as RV;
    use serde_json::Value as JV;
    use std::iter::FromIterator;
    match mval {
        RV::String(s) => JV::String(s.to_string()),
        RV::Boolean(b) => JV::Bool(b),
        RV::Array(v) => JV::Array(Vec::from_iter(v.into_iter().map(msgpack_to_json))),
        RV::F64(f) => JV::from(f),
        RV::F32(f) => JV::from(f),
        RV::Integer(i) => match (i.is_u64(), i.is_i64()) {
            (true, _) => JV::from(i.as_u64().unwrap()),
            (_, true) => JV::from(i.as_i64().unwrap()),
            _ => JV::from(0u64),
        },
        RV::Map(vkv) => JV::Object(serde_json::Map::from_iter(vkv.into_iter().map(|(k, v)| {
            (
                k.as_str().unwrap_or_default().to_string(),
                msgpack_to_json(v),
            )
        }))),
        RV::Binary(v) => match BIN_STR.get().unwrap() {
            's' => JV::String(String::from_utf8_lossy(&v).into_owned()),
            '2' => serde_json::json!({
                "str": String::from_utf8_lossy(&v),
                "bin": v,
            }),
            'b' | _ => JV::Array(Vec::from_iter(v.into_iter().map(JV::from))),
        },
        RV::Ext(i, v) => serde_json::json!({
            "type": i,
            "data": v
        }),
        RV::Nil => JV::Bool(false),
    }
}

/// transform msgpack bytes into json
pub(crate) fn msgpack_to_json_val(msg: Vec<u8>, bin_str: char) -> serde_json::Value {
    use bytes::Buf;

    BIN_STR.set(bin_str).unwrap();

    let bytes = bytes::Bytes::from(msg);
    if let Ok(v) = rmpv::decode::value::read_value(&mut bytes.reader()) {
        msgpack_to_json(v)
    } else {
        serde_json::json!({ "error": "Could not decode data" })
    }
}

pub(crate) fn configure_table_style(table: &mut Table<'_>) {
    table.style = empty_table_style();
    table.separate_rows = false;
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

pub(crate) async fn nats_client_from_opts(
    host: &str,
    port: &str,
    jwt: Option<String>,
    seed: Option<String>,
    credsfile: Option<String>,
) -> Result<Connection> {
    let nats_url = format!("{}:{}", host, port);

    let nc = if let Some(jwt_file) = jwt {
        let jwt_contents = extract_arg_value(&jwt_file)?;
        let kp = if let Some(seed) = seed {
            nkeys::KeyPair::from_seed(&extract_arg_value(&seed)?)?
        } else {
            nkeys::KeyPair::new_user()
        };
        // You must provide the JWT via a closure
        nats::asynk::Options::with_jwt(
            move || Ok(jwt_contents.clone()),
            move |nonce| kp.sign(nonce).unwrap(),
        )
        .connect(&nats_url)
        .await?
    } else if let Some(credsfile_path) = credsfile {
        nats::asynk::Options::with_credentials(credsfile_path)
            .connect(&nats_url)
            .await?
    } else {
        nats::asynk::connect(&nats_url).await?
    };
    Ok(nc)
}
