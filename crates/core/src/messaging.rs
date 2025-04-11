//! Common configuration settings for both the NATS messaging provider and the builtin messaging
//! provider for the host. This module requires the `messaging` feature to be enabled
use std::collections::HashMap;

use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";
pub const CONFIG_NATS_SUBSCRIPTION: &str = "subscriptions";
pub const CONFIG_NATS_CONSUMERS: &str = "consumers";
pub const CONFIG_NATS_URI: &str = "cluster_uris";
pub const CONFIG_NATS_CLIENT_JWT: &str = "client_jwt";
pub const CONFIG_NATS_CLIENT_SEED: &str = "client_seed";
pub const CONFIG_NATS_TLS_CA: &str = "tls_ca";
pub const CONFIG_NATS_CUSTOM_INBOX_PREFIX: &str = "custom_inbox_prefix";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConsumerConfig {
    pub stream: Box<str>,
    pub consumer: Box<str>,
    pub max_messages: Option<usize>,
    pub max_bytes: Option<usize>,
}

/// Configuration for connecting a nats client.
/// More options are available if you use the json than variables in the values string map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectionConfig {
    /// List of topics to subscribe to
    #[serde(default)]
    pub subscriptions: Box<[async_nats::Subject]>,

    /// List of JetStream consumers
    #[serde(default)]
    pub consumers: Box<[ConsumerConfig]>,

    /// Cluster(s) to make a subscription on and connect to
    #[serde(default)]
    pub cluster_uris: Box<[Box<str>]>,

    /// Auth JWT to use (if necessary)
    #[serde(default)]
    pub auth_jwt: Option<Box<str>>,

    /// Auth seed to use (if necessary)
    #[serde(default)]
    pub auth_seed: Option<Box<str>>,

    /// TLS Certificate Authority, encoded as a string
    #[serde(default)]
    pub tls_ca: Option<Box<str>>,

    /// TLS Certificate Authority, as a path on disk
    #[serde(default)]
    pub tls_ca_file: Option<Box<str>>,

    /// Ping interval in seconds
    #[serde(default)]
    pub ping_interval_sec: Option<u16>,

    /// Inbox prefix to use (by default
    #[serde(default)]
    pub custom_inbox_prefix: Option<Box<str>>,
}

impl ConnectionConfig {
    /// Merge a given [`ConnectionConfig`] with another, coalescing fields and overriding
    /// where necessary
    pub fn merge(&self, extra: &ConnectionConfig) -> ConnectionConfig {
        let mut out = self.clone();
        if !extra.subscriptions.is_empty() {
            out.subscriptions.clone_from(&extra.subscriptions);
        }
        if !extra.consumers.is_empty() {
            out.consumers.clone_from(&extra.consumers);
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
            subscriptions: Box::default(),
            consumers: Box::default(),
            cluster_uris: Box::from([DEFAULT_NATS_URI.into()]),
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
    /// Construct configuration from the passed hostdata config
    pub fn from_map(values: &HashMap<String, String>) -> Result<ConnectionConfig> {
        let mut config = ConnectionConfig::default();

        if let Some(sub) = values.get(CONFIG_NATS_SUBSCRIPTION) {
            config.subscriptions = sub.split(',').map(async_nats::Subject::from).collect();
        }
        if let Some(cons) = values.get(CONFIG_NATS_CONSUMERS) {
            config.consumers = serde_json::from_str(cons).context("failed to parse `consumers`")?;
        }
        if let Some(url) = values.get(CONFIG_NATS_URI) {
            config.cluster_uris = url.split(',').map(Box::from).collect();
        }
        if let Some(custom_inbox_prefix) = values.get(CONFIG_NATS_CUSTOM_INBOX_PREFIX) {
            config.custom_inbox_prefix = Some(custom_inbox_prefix.as_str().into());
        }
        if let Some(jwt) = values.get(CONFIG_NATS_CLIENT_JWT) {
            config.auth_jwt = Some(jwt.as_str().into());
        }
        if let Some(seed) = values.get(CONFIG_NATS_CLIENT_SEED) {
            config.auth_seed = Some(seed.as_str().into());
        }
        if let Some(tls_ca) = values.get(CONFIG_NATS_TLS_CA) {
            config.tls_ca = Some(tls_ca.as_str().into());
        }
        if config.auth_jwt.is_some() && config.auth_seed.is_none() {
            bail!("if you specify jwt, you must also specify a seed");
        }

        Ok(config)
    }
}

/// Adds the given CA cert to the provided [`async_nats::ConnectOptions`].
///
/// This follows the builder pattern in that it returns the same `ConnectOptions` with TLS
/// configured
pub fn add_tls_ca(
    tls_ca: &str,
    opts: async_nats::ConnectOptions,
) -> anyhow::Result<async_nats::ConnectOptions> {
    let ca = rustls_pemfile::read_one(&mut tls_ca.as_bytes()).context("failed to read CA")?;
    let mut roots = async_nats::rustls::RootCertStore::empty();
    if let Some(rustls_pemfile::Item::X509Certificate(ca)) = ca {
        roots.add_parsable_certificates([ca]);
    } else {
        bail!("tls ca: invalid certificate type, must be a DER encoded PEM file")
    };
    let tls_client = async_nats::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(opts.tls_client_config(tls_client).require_tls(true))
}
