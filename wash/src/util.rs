use std::collections::HashMap;

pub(crate) type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error>>;

/// Converts error from Send + Sync error to standard error
pub(crate) fn convert_error(
    e: Box<dyn ::std::error::Error + Send + Sync>,
) -> Box<dyn ::std::error::Error> {
    Box::<dyn std::error::Error>::from(format!("{}", e))
}

/// Transforms a list of labels in the form of (label=value) to a hashmap
pub(crate) fn labels_vec_to_hashmap(constraints: Vec<String>) -> Result<HashMap<String, String>> {
    let mut hm: HashMap<String, String> = HashMap::new();
    let mut iter = constraints.iter();
    while let Some(constraint) = iter.next() {
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
