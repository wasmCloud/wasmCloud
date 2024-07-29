use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use tracing::warn;
use wasmcloud_provider_sdk::core::secrets::SecretValue;

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
    /// Construct a [`NatsConnectionConfig`] from a given [`HashMap`] (normally containing a combination of config and secrets)
    ///
    /// Do not use this directly, but instead use [`NatsConnectionConfig::from_link_config`] instead
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

    /// Construct configuration  from a given [`LinkConfig`], utilizing both config and secrets provided
    pub fn from_config_and_secrets(
        config: &HashMap<String, String>,
        secrets: &HashMap<String, SecretValue>,
    ) -> Result<NatsConnectionConfig> {
        let mut map = HashMap::clone(config);

        if let Some(jwt) = secrets
            .get(CONFIG_NATS_CLIENT_JWT)
            .and_then(SecretValue::as_string)
            .or_else(|| config.get(CONFIG_NATS_CLIENT_JWT).map(String::as_str))
        {
            if secrets.get(CONFIG_NATS_CLIENT_JWT).is_none() {
                warn!("secret value [{CONFIG_NATS_CLIENT_JWT}] was missing, but was found configuration. Please prefer using secrets for sensitive values.");
            }
            map.insert(CONFIG_NATS_CLIENT_JWT.into(), jwt.to_string());
        }

        if let Some(seed) = secrets
            .get(CONFIG_NATS_CLIENT_SEED)
            .and_then(SecretValue::as_string)
            .or_else(|| config.get(CONFIG_NATS_CLIENT_SEED).map(String::as_str))
        {
            if secrets.get(CONFIG_NATS_CLIENT_SEED).is_none() {
                warn!("secret value [{CONFIG_NATS_CLIENT_SEED}] was missing, but was found configuration. Please prefer using secrets for sensitive values.");
            }
            map.insert(CONFIG_NATS_CLIENT_SEED.into(), seed.to_string());
        }

        if let Some(tls_ca) = secrets
            .get(CONFIG_NATS_TLS_CA)
            .and_then(SecretValue::as_string)
            .or_else(|| config.get(CONFIG_NATS_TLS_CA).map(String::as_str))
        {
            if secrets.get(CONFIG_NATS_TLS_CA).is_none() {
                warn!("secret value [{CONFIG_NATS_TLS_CA}] was missing, but was found configuration. Please prefer using secrets for sensitive values.");
            }
            map.insert(CONFIG_NATS_TLS_CA.into(), tls_ca.to_string());
        }

        Self::from_map(&map)
    }
}

// Performing various provider configuration tests
#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;

    // Verify that a NatsConnectionConfig could be constructed from partial input
    #[test]
    fn test_default_connection_serialize() {
        let input = r#"
{
    "cluster_uri": "nats://super-cluster",
    "js_domain": "optional",
    "bucket": "kv_store",
    "auth_jwt": "authy",
    "auth_seed": "seedy"
}
"#;

        let config: NatsConnectionConfig = serde_json::from_str(input).unwrap();
        assert_eq!(config.cluster_uri, Some("nats://super-cluster".to_string()));
        assert_eq!(config.js_domain, Some("optional".to_string()));
        assert_eq!(config.bucket, "kv_store");
        assert_eq!(config.auth_jwt.unwrap(), "authy");
        assert_eq!(config.auth_seed.unwrap(), "seedy");
    }

    // Verify that two NatsConnectionConfigs could be merged
    #[test]
    fn test_connectionconfig_merge() {
        let ncc1 = NatsConnectionConfig {
            cluster_uri: Some("old_server".to_string()),
            ..Default::default()
        };
        let ncc2 = NatsConnectionConfig {
            cluster_uri: Some("server1".to_string()),
            js_domain: Some("new_domain".to_string()),
            bucket: "new_bucket".to_string(),
            auth_jwt: Some("jawty".to_string()),
            ..Default::default()
        };
        let ncc3 = ncc1.merge(&ncc2);
        assert_eq!(ncc3.cluster_uri, ncc2.cluster_uri);
        assert_eq!(ncc3.js_domain, ncc2.js_domain);
        assert_eq!(ncc3.bucket, ncc2.bucket);
        assert_eq!(ncc3.auth_jwt, Some("jawty".to_string()));
    }

    // Verify that a NatsConnectionConfig could be constructed from a HashMap
    #[test]
    fn test_from_map_multiple_entries() -> anyhow::Result<()> {
        const CONFIG_NATS_CLIENT_JWT: &str = "client_jwt";
        const CONFIG_NATS_CLIENT_SEED: &str = "client_seed";
        let ncc = NatsConnectionConfig::from_map(&HashMap::from([
            ("tls_ca".to_string(), "rootCA".to_string()),
            ("js_domain".to_string(), "optional".to_string()),
            ("bucket".to_string(), "kv_store".to_string()),
            (CONFIG_NATS_CLIENT_JWT.to_string(), "authy".to_string()),
            (CONFIG_NATS_CLIENT_SEED.to_string(), "seedy".to_string()),
        ]))?;
        assert_eq!(ncc.tls_ca, Some("rootCA".to_string()));
        assert_eq!(ncc.js_domain, Some("optional".to_string()));
        assert_eq!(ncc.bucket, "kv_store");
        assert_eq!(ncc.auth_jwt, Some("authy".to_string()));
        assert_eq!(ncc.auth_seed, Some("seedy".to_string()));
        Ok(())
    }

    // Verify that a default NatsConnectionConfig will be constructed from an empty HashMap
    #[test]
    fn test_from_map_empty() {
        let ncc = NatsConnectionConfig::from_map(&HashMap::new());
        assert!(ncc.is_err());
    }

    // Verify that a NatsConnectionConfig will be constructed from an empty HashMap, plus a required bucket
    #[test]
    fn test_from_map_with_minimal_valid_bucket() -> anyhow::Result<()> {
        let mut map = HashMap::new();
        map.insert("bucket".to_string(), "some_bucket_value".to_string()); // Providing a minimal valid 'bucket' attribute
        let ncc = NatsConnectionConfig::from_map(&map)?;
        assert_eq!(ncc.bucket, "some_bucket_value".to_string());
        Ok(())
    }

    // Verify that the NatsConnectionConfig's merge function prioritizes the new values over the old ones
    #[test]
    fn test_merge_non_default_values() {
        let ncc1 = NatsConnectionConfig {
            cluster_uri: Some("old_server".to_string()),
            js_domain: Some("old_domain".to_string()),
            bucket: "old_bucket".to_string(),
            auth_jwt: Some("old_jawty".to_string()),
            ..Default::default()
        };
        let ncc2 = NatsConnectionConfig {
            cluster_uri: Some("server1".to_string()),
            js_domain: Some("new_domain".to_string()),
            bucket: "kv_store".to_string(),
            auth_jwt: Some("new_jawty".to_string()),
            ..Default::default()
        };
        let ncc3 = ncc1.merge(&ncc2);
        assert_eq!(ncc3.cluster_uri, ncc2.cluster_uri);
        assert_eq!(ncc3.js_domain, ncc2.js_domain);
        assert_eq!(ncc3.bucket, ncc2.bucket);
        assert_eq!(ncc3.auth_jwt, ncc2.auth_jwt);
    }
}
