use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_NATS_URI: &str = "nats://0.0.0.0:4222";

const CONFIG_NATS_URI: &str = "cluster_uri";
const CONFIG_NATS_JETSTREAM_DOMAIN: &str = "js_domain";
const CONFIG_NATS_KV_STORE: &str = "bucket";
const CONFIG_NATS_CLIENT_JWT: &str = "client_jwt";
const CONFIG_NATS_CLIENT_SEED: &str = "client_seed";
const CONFIG_NATS_TLS_CA: &str = "tls_ca";
const CONFIG_NATS_TLS_CA_FILE: &str = "tls_ca_file";

/// Configuration for connecting a NATS client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NatsConnectionConfig {
    /// Cluster(s) to connect to
    #[serde(default)]
    pub cluster_uri: Option<String>,

    /// JetStream Domain to connect to
    #[serde(default)]
    pub js_domain: Option<String>,

    /// NATS Kv Store to open
    #[serde(default)]
    pub bucket: String,

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
}

impl NatsConnectionConfig {
    /// Merge a given [`NatsConnectionConfig`] with another, coalescing fields and overriding
    /// where necessary
    pub fn merge(&self, extra: &NatsConnectionConfig) -> NatsConnectionConfig {
        let mut out = self.clone();
        // If the default configuration has a URI in it, and then the link definition
        // also provides a URI, the assumption is to replace/override rather than combine
        // the two into a potentially incompatible set of URIs
        if extra.cluster_uri.is_some() {
            out.cluster_uri.clone_from(&extra.cluster_uri);
        }
        if extra.js_domain.is_some() {
            out.js_domain.clone_from(&extra.js_domain);
        }
        if !extra.bucket.is_empty() {
            out.bucket.clone_from(&extra.bucket);
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
        out
    }
}

/// Default implementation for [`NatsConnectionConfig`]
impl Default for NatsConnectionConfig {
    fn default() -> NatsConnectionConfig {
        NatsConnectionConfig {
            cluster_uri: Some(DEFAULT_NATS_URI.into()),
            js_domain: None,
            bucket: String::new(),
            auth_jwt: None,
            auth_seed: None,
            tls_ca: None,
            tls_ca_file: None,
        }
    }
}

impl NatsConnectionConfig {
    /// Construct configuration Struct from the passed hostdata config
    pub fn from_map(values: &HashMap<String, String>) -> Result<NatsConnectionConfig> {
        let mut config = NatsConnectionConfig::default();

        if let Some(uri) = values.get(CONFIG_NATS_URI) {
            config.cluster_uri = Some(uri.clone());
        }
        if let Some(domain) = values.get(CONFIG_NATS_JETSTREAM_DOMAIN) {
            config.js_domain = Some(domain.clone());
        }
        if let Some(bucket) = values.get(CONFIG_NATS_KV_STORE) {
            config.bucket.clone_from(bucket);
        } else {
            bail!(
                "missing required configuration item: {}",
                CONFIG_NATS_KV_STORE
            );
        }
        if let Some(jwt) = values.get(CONFIG_NATS_CLIENT_JWT) {
            config.auth_jwt = Some(jwt.clone());
        }
        if let Some(seed) = values.get(CONFIG_NATS_CLIENT_SEED) {
            config.auth_seed = Some(seed.clone());
        }
        if let Some(tls_ca) = values.get(CONFIG_NATS_TLS_CA) {
            config.tls_ca = Some(tls_ca.clone());
        } else if let Some(tls_ca_file) = values.get(CONFIG_NATS_TLS_CA_FILE) {
            config.tls_ca_file = Some(tls_ca_file.clone());
        }
        if config.auth_jwt.is_some() && config.auth_seed.is_none() {
            bail!("if you specify jwt, you must also specify a seed");
        }

        Ok(config)
    }
}
