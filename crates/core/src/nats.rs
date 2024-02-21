use async_nats::HeaderMap;
use std::collections::HashMap;

/// Convert a [`async_nats::HeaderMap`] to a [`HashMap`] of the kind that is used in the smithy contract
/// This method of converting takes the last known value of a given header and uses that as the final value
pub fn convert_header_map_to_hashmap(map: &HeaderMap) -> HashMap<String, String> {
    map.iter()
        .flat_map(|(key, value)| {
            value
                .iter()
                .map(|v| (key.to_string(), v.to_string()))
                .collect::<Vec<_>>()
        })
        .collect::<HashMap<String, String>>()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::convert_header_map_to_hashmap;
    use anyhow::Result;
    use async_nats::HeaderMap;

    /// Ensure that hashmaps only take the last valid header value
    #[test]
    fn test_duplicates() -> Result<()> {
        let mut map = HeaderMap::new();
        map.insert("a", "a");
        map.insert("a", "b");
        map.insert("b", "c");

        assert_eq!(
            convert_header_map_to_hashmap(&map),
            HashMap::from([("a".into(), "b".into()), ("b".into(), "c".into()),]),
        );
        Ok(())
    }
}
