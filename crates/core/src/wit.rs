//! Core reusable functionality related to [WebAssembly Interface types ("WIT")](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)

use std::collections::HashMap;

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

// I don't know if these would be generated or if we'd just include them in the library and then use them in the generated code, but they work around the lack of a map type in wit

/// Representation of maps (AKA associative arrays) that are usable from WIT
///
/// This representation is required because WIT does not natively
/// have support for a map type, so we must use a list of tuples
pub type WitMap<T> = Vec<(String, T)>;

pub(crate) fn serialize_wit_map<S: Serializer, T>(
    map: &WitMap<T>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    T: Serialize,
{
    let mut seq = serializer.serialize_map(Some(map.len()))?;
    for (key, val) in map {
        seq.serialize_entry(key, val)?;
    }
    seq.end()
}

pub(crate) fn deserialize_wit_map<'de, D: serde::Deserializer<'de>, T>(
    deserializer: D,
) -> Result<WitMap<T>, D::Error>
where
    T: Deserialize<'de>,
{
    let values = HashMap::<String, T>::deserialize(deserializer)?;
    Ok(values.into_iter().collect())
}
