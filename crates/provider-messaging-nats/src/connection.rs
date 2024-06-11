use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";

const CONFIG_NATS_SUBSCRIPTION: &str = "subscriptions";
const CONFIG_NATS_URI: &str = "cluster_uris";
const CONFIG_NATS_CLIENT_JWT: &str = "client_jwt";
const CONFIG_NATS_CLIENT_SEED: &str = "client_seed";
const CONFIG_NATS_TLS_CA: &str = "tls_ca";
const CONFIG_NATS_CUSTOM_INBOX_PREFIX: &str = "custom_inbox_prefix";

/// Configuration for connecting a nats client.
/// More options are available if you use the json than variables in the values string map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectionConfig {
    /// List of topics to subscribe to
    #[serde(default)]
    pub subscriptions: Vec<String>,

    /// Cluster(s) to make a subscription on and connect to
    #[serde(default)]
    pub cluster_uris: Vec<String>,

    /// Auth JWT to use (if necessary)
    #[serde(default)]
    pub auth_jwt: Option<String>,

    /// Auth seed to use (if necessary)
    #[serde(default)]
    pub auth_seed: Option<String>,

    /// TLS Certificate Authority, encoded as a string
    #[serde(default)]
    pub tls_ca: Option<String>,

    /// TLS Certifiate Authority, as a path on disk
    #[serde(default)]
    pub tls_ca_file: Option<String>,

    /// Ping interval in seconds
    #[serde(default)]
    pub ping_interval_sec: Option<u16>,

    /// Inbox prefix to use (by default
    #[serde(default)]
    pub custom_inbox_prefix: Option<String>,
}

impl ConnectionConfig {
    /// Merge a given [`ConnectionConfig`] with another, coalescing fields and overriding
    /// where necessary
    pub fn merge(&self, extra: &ConnectionConfig) -> ConnectionConfig {
        let mut out = self.clone();
        if !extra.subscriptions.is_empty() {
            out.subscriptions.clone_from(&extra.subscriptions);
        }
        // If the default configuration has a URL in it, and then the link definition
        // also provides a URL, the assumption is to replace/override rather than combine
        // the two into a potentially incompatible set of URIs
        if !extra.cluster_uris.is_empty() {
            out.cluster_uris.clone_from(&extra.cluster_uris);
        }
        if extra.auth_jwt.is_some() {
            out.auth_jwt.clone_from(&extra.auth_jwt);
        }
        if extra.auth_seed.is_some() {
            out.auth_seed.clone_from(&extra.auth_seed);
        }
        if extra.tls_ca.is_some() {
            out.tls_ca.clone_from(&extra.tls_ca);
        }
        if extra.tls_ca_file.is_some() {
            out.tls_ca_file.clone_from(&extra.tls_ca_file);
        }
        if extra.ping_interval_sec.is_some() {
            out.ping_interval_sec = extra.ping_interval_sec;
        }
        if extra.custom_inbox_prefix.is_some() {
            out.custom_inbox_prefix
                .clone_from(&extra.custom_inbox_prefix);
        }
        out
    }
}

impl Default for ConnectionConfig {
    fn default() -> ConnectionConfig {
        ConnectionConfig {
            subscriptions: vec![],
            cluster_uris: vec![DEFAULT_NATS_URI.into()],
            auth_jwt: None,
            auth_seed: None,
            tls_ca: None,
            tls_ca_file: None,
            ping_interval_sec: None,
            custom_inbox_prefix: None,
        }
    }
}

impl ConnectionConfig {
    /// Construct configuration Struct from the passed hostdata config
    pub fn from_map(values: &HashMap<String, String>) -> Result<ConnectionConfig> {
        let mut config = ConnectionConfig::default();

        if let Some(sub) = values.get(CONFIG_NATS_SUBSCRIPTION) {
            config
                .subscriptions
                .extend(sub.split(',').map(std::string::ToString::to_string));
        }
        if let Some(url) = values.get(CONFIG_NATS_URI) {
            config.cluster_uris = url.split(',').map(String::from).collect();
        }
        if let Some(custom_inbox_prefix) = values.get(CONFIG_NATS_CUSTOM_INBOX_PREFIX) {
            config.custom_inbox_prefix = Some(custom_inbox_prefix.clone());
        }
        if let Some(jwt) = values.get(CONFIG_NATS_CLIENT_JWT) {
            config.auth_jwt = Some(jwt.clone());
        }
        if let Some(seed) = values.get(CONFIG_NATS_CLIENT_SEED) {
            config.auth_seed = Some(seed.clone());
        }
        if let Some(tls_ca) = values.get(CONFIG_NATS_TLS_CA) {
            config.tls_ca = Some(tls_ca.clone());
        }
        if config.auth_jwt.is_some() && config.auth_seed.is_none() {
            bail!("if you specify jwt, you must also specify a seed");
        }

        Ok(config)
    }
}
