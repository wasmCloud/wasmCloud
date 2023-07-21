use std::collections::HashMap;

/// turns Hashmap into case-insensitive hashmap by making all keys lowercase.
/// Values in returned hashmap are still owned by original, to avoid copying.
/// If there are any key collisions, (e.g.,"Key" and "KEY" both map to "key"),
/// returns None.
pub(crate) fn make_case_insensitive<T>(inp: &HashMap<String, T>) -> Option<HashMap<String, &T>> {
    let orig_size = inp.len();
    let out: HashMap<String, &T> = inp
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v))
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
        let mut map = HashMap::new();
        map.insert("One".to_string(), "x");
        map.insert("TWO".to_string(), "y");
        map.insert("three".to_string(), "z");

        let ci = make_case_insensitive(&map);
        assert!(ci.is_some());
        let ci = ci.unwrap();
        assert_eq!(ci.get("one"), Some(&&"x"), "One -> one");
        assert_eq!(ci.get("One"), None, "original key not there");
        assert_eq!(ci.get("two"), Some(&&"y"), "TWO -> two");
        assert_eq!(ci.get("three"), Some(&&"z"), "three unchanged");
    }

    #[test]
    fn detect_collisions() {
        let mut map = HashMap::new();
        map.insert("One".to_string(), "x");
        map.insert("ONE".to_string(), "y");

        let ci = make_case_insensitive(&map);
        assert!(ci.is_none());
    }
}
