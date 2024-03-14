use std::{fs::File, io::Read};

use anyhow::{Context, Result};
use term_table::{Table, TableStyle};
use wash_lib::config::DEFAULT_NATS_TIMEOUT_MS;

pub fn format_optional(value: Option<String>) -> String {
    value.unwrap_or_else(|| "N/A".into())
}

/// Returns value from an argument that may be a file path or the value itself
pub fn extract_arg_value(arg: &str) -> Result<String> {
    match File::open(arg) {
        Ok(mut f) => {
            let mut value = String::new();
            f.read_to_string(&mut value)
                .with_context(|| format!("Failed to read file {}", &arg))?;
            Ok(value)
        }
        Err(_) => Ok(arg.to_string()),
    }
}

pub fn default_timeout_ms() -> u64 {
    DEFAULT_NATS_TIMEOUT_MS
}

/// Transform a json string (e.g. "{"hello": "world"}") into msgpack bytes
pub fn json_str_to_msgpack_bytes(payload: &str) -> Result<Vec<u8>> {
    let json: serde_json::Value =
        serde_json::from_str(payload).context("failed to encode string as JSON")?;
    rmp_serde::to_vec_named(&json).context("failed to encode JSON as msgpack")
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
pub fn msgpack_to_json_val(msg: Vec<u8>, bin_str: char) -> serde_json::Value {
    use bytes::Buf;

    BIN_STR.set(bin_str).unwrap();

    let bytes = bytes::Bytes::from(msg);
    if let Ok(v) = rmpv::decode::value::read_value(&mut bytes.reader()) {
        msgpack_to_json(v)
    } else {
        serde_json::json!({ "error": "Could not decode data" })
    }
}

pub fn configure_table_style(table: &mut Table<'_>) {
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

mod test {
    #[test]
    fn test_safe_base64_parse_option() {
        let base64_option = "config_b64=eyJhZGRyZXNzIjogIjAuMC4wLjA6ODA4MCJ9Cg==".to_string();
        let mut expected = std::collections::HashMap::new();
        expected.insert(
            "config_b64".to_string(),
            "eyJhZGRyZXNzIjogIjAuMC4wLjA6ODA4MCJ9Cg==".to_string(),
        );
        let output = wash_lib::cli::input_vec_to_hashmap(vec![base64_option]).unwrap();
        assert_eq!(expected, output);
    }
}
