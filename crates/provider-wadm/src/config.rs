use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";

const CONFIG_NATS_URI: &str = "cluster_uris";
const CONFIG_NATS_CLIENT_JWT: &str = "client_jwt";
const CONFIG_NATS_CLIENT_SEED: &str = "client_seed";
const CONFIG_NATS_TLS_CA: &str = "tls_ca";
const CONFIG_NATS_TLS_CA_FILE: &str = "tls_ca_file";
const CONFIG_NATS_PING_INTERVAL_SEC: &str = "ping_interval_sec";

/// The NATS prefix wadm's API is listening on
pub const WADM_API_PREFIX: &str = "wadm.api";

/// Configuration for connecting a NATS client.
/// More options are available if you use the JSON than variables in the values string map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WadmProviderConfig {
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

    /// TLS Certificate Authority, as a path on disk
    #[serde(default)]
    pub tls_ca_file: Option<String>,

    /// Ping interval in seconds
    #[serde(default)]
    pub ping_interval_sec: Option<u16>,

    /// Inbox prefix to use (by default)
    #[serde(default)]
    pub custom_inbox_prefix: Option<String>,
}

impl Default for WadmProviderConfig {
    fn default() -> Self {
        WadmProviderConfig {
            subscriptions: Vec::new(),
            cluster_uris: vec![DEFAULT_NATS_URI.to_string()],
            auth_jwt: None,
            auth_seed: None,
            tls_ca: None,
            tls_ca_file: None,
            ping_interval_sec: None,
            custom_inbox_prefix: None,
        }
    }
}

impl WadmProviderConfig {
    /// Merge a given [`WadmProviderConfig`] with another, coalescing fields and overriding
    /// where necessary
    pub fn merge(&self, extra: &WadmProviderConfig) -> WadmProviderConfig {
        let mut out = self.clone();

        if !extra.cluster_uris.is_empty() {
            out.cluster_uris = extra.cluster_uris.clone();
        }
        if extra.auth_jwt.is_some() {
            out.auth_jwt = extra.auth_jwt.clone();
        }
        if extra.auth_seed.is_some() {
            out.auth_seed = extra.auth_seed.clone();
        }
        if extra.tls_ca.is_some() {
            out.tls_ca = extra.tls_ca.clone();
        }
        if extra.tls_ca_file.is_some() {
            out.tls_ca_file = extra.tls_ca_file.clone();
        }
        if extra.ping_interval_sec.is_some() {
            out.ping_interval_sec = extra.ping_interval_sec;
        }

        out
    }
}

impl WadmProviderConfig {
    /// Construct configuration Struct from the passed hostdata config
    pub fn from_map(values: &HashMap<String, String>) -> Result<WadmProviderConfig> {
        let mut config = WadmProviderConfig::default();

        // TODO - does this need to be on subscription?
        // NOTE: for now extra subscriptions are not supported, and
        // default to just the wadm api
        config.subscriptions = vec![WADM_API_PREFIX.to_string()];

        if let Some(cluster_uris) = values.get(CONFIG_NATS_URI) {
            config.cluster_uris = cluster_uris.split(',').map(String::from).collect();
        }
        if let Some(auth_jwt) = values.get(CONFIG_NATS_CLIENT_JWT) {
            config.auth_jwt = Some(auth_jwt.clone());
        }
        if let Some(auth_seed) = values.get(CONFIG_NATS_CLIENT_SEED) {
            config.auth_seed = Some(auth_seed.clone());
        }
        if let Some(tls_ca) = values.get(CONFIG_NATS_TLS_CA) {
            config.tls_ca = Some(tls_ca.clone());
        }
        if let Some(tls_ca_file) = values.get(CONFIG_NATS_TLS_CA_FILE) {
            config.tls_ca_file = Some(tls_ca_file.clone());
        }
        if let Some(ping_interval_sec) = values.get(CONFIG_NATS_PING_INTERVAL_SEC) {
            config.ping_interval_sec = Some(ping_interval_sec.parse().unwrap());
        }

        Ok(config)
    }
}
