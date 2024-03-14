use std::collections::HashMap;

/// turns Hashmap into case-insensitive hashmap by making all keys lowercase.
/// Values in returned hashmap are still owned by original, to avoid copying.
/// If there are any key collisions, (e.g.,"Key" and "KEY" both map to "key"),
/// returns None.
pub(crate) fn make_case_insensitive(
    inp: &HashMap<String, String>,
) -> Option<HashMap<String, String>> {
    let orig_size = inp.len();
    let out: HashMap<String, String> = inp
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
        .collect();
    if out.len() != orig_size {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn map_case_insensitive() {
        let map = HashMap::from([
            ("One".to_string(), "x".to_string()),
            ("TWO".to_string(), "y".to_string()),
            ("three".to_string(), "z".to_string()),
        ]);

        let ci = make_case_insensitive(&map);
        assert!(ci.is_some());
        let ci = ci.unwrap();
        assert_eq!(ci.get("one"), Some(&"x".to_string()), "One -> one");
        assert_eq!(ci.get("One"), None, "original key not there");
        assert_eq!(ci.get("two"), Some(&"y".to_string()), "TWO -> two");
        assert_eq!(ci.get("three"), Some(&"z".to_string()), "three unchanged");
    }

    #[test]
    fn detect_collisions() {
        let map = HashMap::from([
            ("One".to_string(), "x".to_string()),
            ("ONE".to_string(), "y".to_string()),
        ]);

        let ci = make_case_insensitive(&map);
        assert!(ci.is_none());
    }
}
