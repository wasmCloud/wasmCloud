use std::collections::HashMap;

use serde::{Deserialize, Serialize};

const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";

const CONFIG_NATS_SUBSCRIPTION: &str = "subscriptions";
const CONFIG_NATS_URI: &str = "uri";

/// Configuration for connecting a NATS client.
/// More options are available if you use the json than variables in the values string map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectionConfig {
    /// List of topics to subscribe to for a given component
    #[serde(default)]
    pub subscriptions: Vec<String>,

    /// URI for the NATS cluster to connect to
    #[serde(default)]
    pub uri: String,
}

impl ConnectionConfig {
    /// Merge a given [`ConnectionConfig`] with another, coalescing fields and overriding
    /// where necessary
    pub fn merge(&self, extra: ConnectionConfig) -> ConnectionConfig {
        let mut out = self.clone();
        if !extra.subscriptions.is_empty() {
            out.subscriptions = extra.subscriptions;
        }
        // If the default configuration has a URL in it, and then the link definition
        // also provides a URL, the assumption is to replace/override rather than combine
        // the two into a potentially incompatible set of URIs
        if !extra.uri.is_empty() {
            out.uri = extra.uri;
        }
        out
    }
}

impl Default for ConnectionConfig {
    fn default() -> ConnectionConfig {
        ConnectionConfig {
            subscriptions: vec![],
            uri: DEFAULT_NATS_URI.to_string(),
        }
    }
}

impl From<&HashMap<String, String>> for ConnectionConfig {
    /// Construct configuration Struct from the passed config values
    fn from(values: &HashMap<String, String>) -> ConnectionConfig {
        let mut config = ConnectionConfig::default();

        if let Some(sub) = values.get(CONFIG_NATS_SUBSCRIPTION) {
            config
                .subscriptions
                .extend(sub.split(',').map(|s| s.to_string()));
        }
        if let Some(uri) = values.get(CONFIG_NATS_URI) {
            config.uri = uri.to_string();
        }

        config
    }
}
