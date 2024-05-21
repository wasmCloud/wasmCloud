use std::collections::HashMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";
const DEFAULT_LATTICE: &str = "default";

const CONFIG_NATS_URI: &str = "cluster_uris";
const CONFIG_NATS_CLIENT_JWT: &str = "client_jwt";
const CONFIG_NATS_CLIENT_SEED: &str = "client_seed";
const CONFIG_NATS_TLS_CA: &str = "tls_ca";
const CONFIG_NATS_TLS_CA_FILE: &str = "tls_ca_file";
const CONFIG_NATS_PING_INTERVAL_SEC: &str = "ping_interval_sec";
const CONFIG_CUSTOM_INBOX_PREFIX: &str = "custom_inbox_prefix";
const CONFIG_LATTICE: &str = "lattice";
const CONFIG_APP_NAME: &str = "app_name";

pub const WADM_STATUS_API_PREFIX: &str = "wadm.status";

fn default_lattice() -> String {
    DEFAULT_LATTICE.to_string()
}

/// Configuration for interacting with WADM over NATS.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WadmConfig {
    /// Lattice to subscribe to
    #[serde(default = "default_lattice")]
    pub lattice: String,

    /// Application name to subscribe to updates for
    pub app_name: String,

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

impl Default for WadmConfig {
    fn default() -> Self {
        WadmConfig {
            lattice: default_lattice(),
            app_name: String::new(),
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

impl WadmConfig {
    /// Merge a given [`WadmConfig`] with another, coalescing fields and overriding
    /// where necessary
    pub fn merge(&self, extra: &WadmConfig) -> WadmConfig {
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
        if extra.custom_inbox_prefix.is_some() {
            out.custom_inbox_prefix = extra.custom_inbox_prefix.clone();
        }
        if !extra.lattice.is_empty() {
            out.lattice = extra.lattice.clone();
        }
        if !extra.app_name.is_empty() {
            out.app_name = extra.app_name.clone();
        }

        out
    }

    /// Construct configuration Struct from the passed hostdata config
    pub fn from_map(values: &HashMap<String, String>) -> Result<WadmConfig> {
        let mut config = WadmConfig::default();

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
        if let Some(custom_inbox_prefix) = values.get(CONFIG_CUSTOM_INBOX_PREFIX) {
            config.custom_inbox_prefix = Some(custom_inbox_prefix.clone());
        }
        if let Some(lattice) = values.get(CONFIG_LATTICE) {
            config.lattice = lattice.clone();
        }
        if let Some(app_name) = values.get(CONFIG_APP_NAME) {
            config.app_name = app_name.clone();
        }

        Ok(config)
    }
}
