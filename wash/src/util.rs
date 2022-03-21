use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env::temp_dir,
    error::Error,
    fmt, fs,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    str::FromStr,
};
use term_table::{Table, TableStyle};
use wasmbus_rpc::anats;

pub const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub const DEFAULT_NATS_PORT: &str = "4222";
pub const DEFAULT_LATTICE_PREFIX: &str = "default";
pub const DEFAULT_NATS_TIMEOUT_MS: u64 = 2_000;
pub const DEFAULT_START_PROVIDER_TIMEOUT_MS: u64 = 60_000;

/// Used for displaying human-readable output vs JSON format
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum OutputKind {
    Text,
    Json,
}

impl FromStr for OutputKind {
    type Err = OutputParseErr;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "json" => Ok(OutputKind::Json),
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

pub(crate) struct CommandOutput {
    pub map: std::collections::HashMap<String, serde_json::Value>,
    pub text: String,
}

impl CommandOutput {
    pub(crate) fn new(
        text: String,
        map: std::collections::HashMap<String, serde_json::Value>,
    ) -> Self {
        CommandOutput { map, text }
    }

    /// shorthand to create a new CommandOutput with a single key-value pair for JSON, and simply the text for text output.
    pub fn from_key_and_text(key: &str, text: String) -> Self {
        let mut map = std::collections::HashMap::new();
        map.insert(key.to_string(), serde_json::Value::String(text.clone()));
        CommandOutput { map, text }
    }
}

impl From<String> for CommandOutput {
    /// Create a basic CommandOutput from a String. Puts the string a a "result" key in the JSON output.
    fn from(text: String) -> Self {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "result".to_string(),
            serde_json::Value::String(text.clone()),
        );
        CommandOutput { map, text }
    }
}

impl From<&str> for CommandOutput {
    /// Create a basic CommandOutput from a &str. Puts the string a a "result" key in the JSON output.
    fn from(text: &str) -> Self {
        CommandOutput::from(text.to_string())
    }
}

impl Default for CommandOutput {
    fn default() -> Self {
        CommandOutput {
            map: std::collections::HashMap::new(),
            text: "".to_string(),
        }
    }
}

pub(crate) fn default_timeout_ms() -> u64 {
    DEFAULT_NATS_TIMEOUT_MS
}

/// Converts error from Send + Sync error to standard anyhow error
pub(crate) fn convert_error(e: Box<dyn ::std::error::Error + Send + Sync>) -> anyhow::Error {
    anyhow!(e.to_string())
}

/// Transforms a list of labels in the form of (label=value) to a hashmap
pub(crate) fn labels_vec_to_hashmap(constraints: Vec<String>) -> Result<HashMap<String, String>> {
    let mut hm: HashMap<String, String> = HashMap::new();
    for constraint in constraints {
        match constraint.split_once('=') {
            Some((key, value)) => {
                hm.insert(key.to_string(), value.to_string());
            }
            None => {
                bail!("Constraints were not properly formatted. Ensure they are formatted as label=value")
            }
        };
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
    match mval {
        RV::String(s) => JV::String(s.to_string()),
        RV::Boolean(b) => JV::Bool(b),
        RV::Array(v) => JV::Array(v.into_iter().map(msgpack_to_json).collect::<Vec<_>>()),
        RV::F64(f) => JV::from(f),
        RV::F32(f) => JV::from(f),
        RV::Integer(i) => match (i.is_u64(), i.is_i64()) {
            (true, _) => JV::from(i.as_u64().unwrap()),
            (_, true) => JV::from(i.as_i64().unwrap()),
            _ => JV::from(0u64),
        },
        RV::Map(vkv) => JV::Object(
            vkv.into_iter()
                .map(|(k, v)| {
                    (
                        k.as_str().unwrap_or_default().to_string(),
                        msgpack_to_json(v),
                    )
                })
                .collect::<serde_json::Map<_, _>>(),
        ),
        RV::Binary(v) => match BIN_STR.get().unwrap() {
            's' => JV::String(String::from_utf8_lossy(&v).into_owned()),
            '2' => serde_json::json!({
                "str": String::from_utf8_lossy(&v),
                "bin": v,
            }),
            /*'b'|*/ _ => JV::Array(v.into_iter().map(JV::from).collect::<Vec<_>>()),
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
    credsfile: Option<PathBuf>,
) -> Result<anats::Connection> {
    let nats_url = format!("{}:{}", host, port);

    let nc = if let Some(jwt_file) = jwt {
        let jwt_contents = extract_arg_value(&jwt_file)?;
        let kp = if let Some(seed) = seed {
            nkeys::KeyPair::from_seed(&extract_arg_value(&seed)?)?
        } else {
            nkeys::KeyPair::new_user()
        };

        // You must provide the JWT via a closure
        anats::Options::with_jwt(
            move || Ok(jwt_contents.clone()),
            move |nonce| kp.sign(nonce).unwrap(),
        )
        .connect(&nats_url)
        .await?
    } else if let Some(seed) = seed {
        let kp = nkeys::KeyPair::from_seed(&extract_arg_value(&seed)?)?;
        anats::Options::with_nkey(&kp.public_key(), move |nonce| kp.sign(nonce).unwrap())
            .connect(&nats_url)
            .await?
    } else if let Some(credsfile_path) = credsfile {
        anats::Options::with_credentials(credsfile_path)
            .connect(&nats_url)
            .await?
    } else {
        anats::connect(&nats_url).await?
    };
    Ok(nc)
}

pub(crate) const OCI_CACHE_DIR: &str = "wasmcloud_ocicache";

pub(crate) fn cached_file(img: &str) -> PathBuf {
    let path = temp_dir();
    let path = path.join(OCI_CACHE_DIR);
    let _ = ::std::fs::create_dir_all(&path);
    // should produce a file like wasmcloud_azurecr_io_kvcounter_v1.bin
    let mut path = path.join(img_name_to_file_name(img));
    path.set_extension("bin");

    path
}

pub(crate) fn img_name_to_file_name(img: &str) -> String {
    img.replace(':', "_").replace('/', "_").replace('.', "_")
}

// Check if the contract ID parameter is a 56 character key and suggest that the user
// give the contract ID instead
//
// NOTE: `len` is ok here because keys are only ascii characters that take up a single
// byte.
pub fn validate_contract_id(contract_id: &str) -> Result<()> {
    if contract_id.len() == 56
        && contract_id
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    {
        bail!("It looks like you used an Actor or Provider ID (e.g. VABC...) instead of a contract ID (e.g. wasmcloud:httpserver)")
    } else {
        Ok(())
    }
}

#[cfg(all(unix))]
/// Set file and folder permissions for keys.
pub(crate) fn set_permissions_keys(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = path.metadata()?;
    match metadata.file_type().is_dir() {
        true => fs::set_permissions(path, fs::Permissions::from_mode(0o700))?,
        false => fs::set_permissions(path, fs::Permissions::from_mode(0o600))?,
    };
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn set_permissions_keys(path: &Path) -> Result<()> {
    Ok(())
}

mod test {
    use super::labels_vec_to_hashmap;
    use std::collections::HashMap;
    #[test]
    fn test_safe_base64_parse_option() {
        let base64_option = "config_b64=eyJhZGRyZXNzIjogIjAuMC4wLjA6ODA4MCJ9Cg==".to_string();
        let mut expected = HashMap::new();
        expected.insert(
            "config_b64".to_string(),
            "eyJhZGRyZXNzIjogIjAuMC4wLjA6ODA4MCJ9Cg==".to_string(),
        );
        let output = labels_vec_to_hashmap(vec![base64_option]).unwrap();
        assert_eq!(expected, output);
    }
}
