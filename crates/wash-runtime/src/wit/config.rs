//! An interface's config: text, or a plugin secret that stays one until the
//! plugin reading it asks for its text.

use std::collections::{BTreeMap, HashMap, btree_map};

use serde::{Deserialize, Serialize};

use crate::plugin::SecretValue;

/// One config value.
#[derive(Clone, PartialEq, Eq)]
pub enum ConfigValue {
    /// Written by an operator or a manifest.
    Text(String),
    /// Resolved from a plugin-backed `secrets:` entry at bind.
    Secret(SecretValue),
}

impl ConfigValue {
    /// Whether this is a plugin secret.
    #[must_use]
    pub fn is_secret(&self) -> bool {
        matches!(self, Self::Secret(_))
    }

    /// The text, or `None` for a secret.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Secret(_) => None,
        }
    }

    /// The value for reading, still redacted when formatted. `None` for a
    /// secret that is not UTF-8.
    #[must_use]
    pub fn as_config_str(&self) -> Option<ConfigStr<'_>> {
        match self {
            Self::Text(text) => Some(ConfigStr {
                text,
                secret: false,
            }),
            Self::Secret(secret) => secret.expose().map(|text| ConfigStr { text, secret: true }),
        }
    }

    /// The text of either kind, `None` for a secret that is not UTF-8. Never
    /// log or quote it.
    #[must_use]
    pub fn expose(&self) -> Option<&str> {
        self.as_config_str().map(ConfigStr::expose)
    }
}

impl From<String> for ConfigValue {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for ConfigValue {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

impl From<&String> for ConfigValue {
    fn from(text: &String) -> Self {
        Self::Text(text.clone())
    }
}

impl From<SecretValue> for ConfigValue {
    fn from(secret: SecretValue) -> Self {
        Self::Secret(secret)
    }
}

/// Compares the text of either kind, without exposing it.
impl PartialEq<&str> for ConfigValue {
    fn eq(&self, other: &&str) -> bool {
        self.expose() == Some(*other)
    }
}

impl PartialEq<str> for ConfigValue {
    fn eq(&self, other: &str) -> bool {
        self.expose() == Some(other)
    }
}

impl std::fmt::Debug for ConfigValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(text) => f.debug_tuple("Text").field(text).finish(),
            Self::Secret(_) => f.write_str("Secret(<redacted>)"),
        }
    }
}

/// Text, or `<redacted>` for a secret.
impl std::fmt::Display for ConfigValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Secret(_) => f.write_str("<redacted>"),
        }
    }
}

/// A config value being read. Formats as its text, or as `<redacted>` for a
/// secret, so a parser quoting the value it refuses cannot leak one; reading
/// the text itself is [`Self::expose`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ConfigStr<'a> {
    text: &'a str,
    secret: bool,
}

impl<'a> ConfigStr<'a> {
    /// The text. Never log or quote it: format `self` instead.
    #[must_use]
    pub fn expose(self) -> &'a str {
        self.text
    }

    /// Whether this came from a plugin secret.
    #[must_use]
    pub fn is_secret(self) -> bool {
        self.secret
    }

    /// Without leading and trailing whitespace, and still a secret if it was.
    #[must_use]
    pub fn trim(self) -> Self {
        Self {
            text: self.text.trim(),
            ..self
        }
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.text.is_empty()
    }

    /// `result` of handing this value to code that may quote its input. For a
    /// secret, a failure keeps none of that error and names only `key`.
    pub fn guard<T, E: Into<anyhow::Error>>(
        self,
        key: &str,
        result: Result<T, E>,
    ) -> anyhow::Result<T> {
        match result {
            Ok(value) => Ok(value),
            Err(_) if self.secret => Err(anyhow::anyhow!(
                "the secret in `{key}` was refused; the reason is withheld because it may quote \
                 the value"
            )),
            Err(e) => Err(e.into()),
        }
    }
}

impl<'a> From<&'a str> for ConfigStr<'a> {
    fn from(text: &'a str) -> Self {
        Self {
            text,
            secret: false,
        }
    }
}

impl std::fmt::Display for ConfigStr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.secret {
            f.write_str("<redacted>")
        } else {
            f.write_str(self.text)
        }
    }
}

impl std::fmt::Debug for ConfigStr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.secret {
            f.write_str("<redacted>")
        } else {
            std::fmt::Debug::fmt(self.text, f)
        }
    }
}

/// An interface's config, keyed by config key.
///
/// `Debug` shows keys only: a [`ConfigValue::Text`] may be credentials an
/// operator resolved from a Kubernetes Secret. `Serialize` writes each secret
/// as `<redacted>`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Config(BTreeMap<String, ConfigValue>);

impl Config {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The text at `key`, or `None` when it is unset or a secret: a reader
    /// that takes a secret reads it through [`Self::value`].
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(ConfigValue::as_text)
    }

    /// The value at `key` of either kind, formatting a secret as
    /// `<redacted>`.
    #[must_use]
    pub fn value(&self, key: &str) -> Option<ConfigStr<'_>> {
        self.0.get(key).and_then(ConfigValue::as_config_str)
    }

    /// The text at `key` of either kind. Never log or quote it.
    #[must_use]
    pub fn expose(&self, key: &str) -> Option<&str> {
        self.value(key).map(ConfigStr::expose)
    }

    #[must_use]
    pub fn get_value(&self, key: &str) -> Option<&ConfigValue> {
        self.0.get(key)
    }

    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn insert(
        &mut self,
        key: impl Into<String>,
        value: impl Into<ConfigValue>,
    ) -> Option<ConfigValue> {
        self.0.insert(key.into(), value.into())
    }

    pub fn remove(&mut self, key: &str) -> Option<ConfigValue> {
        self.0.remove(key)
    }

    pub fn retain(&mut self, f: impl FnMut(&String, &mut ConfigValue) -> bool) {
        self.0.retain(f);
    }

    pub fn entry(&mut self, key: String) -> btree_map::Entry<'_, String, ConfigValue> {
        self.0.entry(key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn keys(&self) -> btree_map::Keys<'_, String, ConfigValue> {
        self.0.keys()
    }

    /// Every entry in key order; [`ConfigValue::is_secret`] tells them apart.
    pub fn iter(&self) -> btree_map::Iter<'_, String, ConfigValue> {
        self.0.iter()
    }

    /// Every entry with each secret written as `<redacted>`, for a text-only
    /// destination that is not the consumer of this config.
    #[must_use]
    pub fn redacted(&self) -> BTreeMap<String, String> {
        self.0
            .iter()
            .map(|(k, v)| (k.clone(), v.to_string()))
            .collect()
    }
}

/// Test-only, as a map's: the crate reads through [`Config::get`] and
/// [`Config::value`].
#[cfg(test)]
#[allow(clippy::indexing_slicing)]
impl std::ops::Index<&str> for Config {
    type Output = ConfigValue;

    fn index(&self, key: &str) -> &ConfigValue {
        &self.0[key]
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.0.keys().map(|k| (k, format_args!("<redacted>"))))
            .finish()
    }
}

impl Serialize for Config {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(k, v)| (k, v.to_string())))
    }
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BTreeMap::<String, String>::deserialize(deserializer).map(Self::from)
    }
}

impl<K: Into<String>, V: Into<ConfigValue>> FromIterator<(K, V)> for Config {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl<K: Into<String>, V: Into<ConfigValue>> Extend<(K, V)> for Config {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        self.0
            .extend(iter.into_iter().map(|(k, v)| (k.into(), v.into())));
    }
}

impl IntoIterator for Config {
    type Item = (String, ConfigValue);
    type IntoIter = btree_map::IntoIter<String, ConfigValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Config {
    type Item = (&'a String, &'a ConfigValue);
    type IntoIter = btree_map::Iter<'a, String, ConfigValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl From<HashMap<String, String>> for Config {
    fn from(map: HashMap<String, String>) -> Self {
        map.into_iter().collect()
    }
}

impl From<BTreeMap<String, String>> for Config {
    fn from(map: BTreeMap<String, String>) -> Self {
        map.into_iter().collect()
    }
}

impl<K: Into<String>, V: Into<ConfigValue>, const N: usize> From<[(K, V); N]> for Config {
    fn from(pairs: [(K, V); N]) -> Self {
        pairs.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let mut config: Config = [("url", "postgres://db")].into_iter().collect();
        config.insert("password", SecretValue::new("hunter2"));
        config
    }

    #[test]
    fn get_reads_text_and_value_reads_either() {
        let config = config();
        assert_eq!(config.get("url"), Some("postgres://db"));
        assert_eq!(config.get("password"), None);
        assert_eq!(config.expose("password"), Some("hunter2"));
        let password = config.value("password").unwrap();
        assert!(password.is_secret());
        assert_eq!(password.expose(), "hunter2");
    }

    #[test]
    fn formatting_never_prints_a_secret() {
        let config = config();
        let password = config.value("password").unwrap();
        for rendered in [
            format!("{password}"),
            format!("{password:?}"),
            format!("{}", password.trim()),
            format!("{config:?}"),
            format!("{:?}", config.get_value("password").unwrap()),
            format!("{}", config.get_value("password").unwrap()),
            serde_json::to_string(&config).unwrap(),
        ] {
            assert!(!rendered.contains("hunter2"), "{rendered}");
        }
        assert_eq!(
            serde_json::to_string(&config).unwrap(),
            r#"{"password":"<redacted>","url":"postgres://db"}"#
        );
    }

    #[test]
    fn deserializes_as_text() {
        let config: Config = serde_json::from_str(r#"{"a":"b"}"#).unwrap();
        assert_eq!(config.get("a"), Some("b"));
        assert!(!config.get_value("a").unwrap().is_secret());
    }

    #[test]
    fn guard_keeps_a_text_error_and_drops_a_secret_one() {
        let refused = || anyhow::bail!("cannot parse `hunter2`") as anyhow::Result<()>;

        let text = ConfigStr::from("hunter2");
        let err = text.guard("password", refused()).unwrap_err();
        assert!(err.to_string().contains("cannot parse `hunter2`"), "{err}");

        let secret = config();
        let err = secret
            .value("password")
            .unwrap()
            .guard("password", refused())
            .unwrap_err();
        assert!(!format!("{err:#}").contains("hunter2"), "{err:#}");
        assert!(err.to_string().contains("`password`"), "{err}");

        assert_eq!(text.guard("password", anyhow::Ok(1)).unwrap(), 1);
    }
}
