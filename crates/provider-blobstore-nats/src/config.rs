use std::collections::HashMap;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use tracing::warn;
use wasmcloud_provider_sdk::core::secrets::SecretValue;

pub(crate) const DEFAULT_NATS_URI: &str = "nats://0.0.0.0:4222";

const CONFIG_NATS_URI: &str = "cluster_uri";
const CONFIG_NATS_JETSTREAM_DOMAIN: &str = "js_domain";
const CONFIG_NATS_CLIENT_JWT: &str = "client_jwt";
const CONFIG_NATS_CLIENT_SEED: &str = "client_seed";
const CONFIG_NATS_TLS_CA: &str = "tls_ca";
const CONFIG_NATS_TLS_CA_FILE: &str = "tls_ca_file";
const CONFIG_NATS_MAX_WRITE_WAIT: &str = "max_write_wait";
const CONFIG_NATS_STORAGE_MAX_AGE: &str = "max_age";
const CONFIG_NATS_STORAGE_TYPE: &str = "storage_type";
const CONFIG_NATS_STORAGE_NUM_REPLICAS: &str = "num_replicas";
const CONFIG_NATS_STORAGE_COMPRESSION: &str = "compression";

/// Configuration for connecting a NATS client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NatsConnectionConfig {
    /// Cluster(s) to connect to
    #[serde(default)]
    pub cluster_uri: Option<String>,

    /// JetStream Domain to connect to
    #[serde(default)]
    pub js_domain: Option<String>,

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

    /// Backend storage configuration (opaque service provider specific configuration)
    #[serde(default)]
    pub storage_config: Option<StorageConfig>,

    /// Write operation timeout configuration
    #[serde(default)]
    pub max_write_wait: Option<u64>,
}

/// NATS Object-store is backed by its Key-Value store; so an object-store
/// is identified by its backing bucket name.
///
/// Note that when storage config is provided via link configuration
/// the following keys are expected:
/// - `max_age` (optional): the maximum age of any object in the container, expressed in seconds; defaults to 10 years
/// - `storage_type` (optional): the type of storage backend, File (default) and Memory
/// - `num_replicas` (optional): how many replicas to keep for each object in a cluster, maximum 5; defaults to 1
/// - `compression` (optional): whether the underlying stream should be compressed; defaults to false
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StorageConfig {
    /// Maximum age of any object in the container, expressed in nanoseconds
    #[serde(default)]
    pub max_age: core::time::Duration,
    /// Maximum size of the object store container, expressed in bytes
    #[serde(default)]
    pub max_bytes: i64,
    /// The type of storage backend, File (default) and Memory
    #[serde(default)]
    pub storage_type: StorageType,
    /// How many replicas to keep for each object in a cluster, maximum 5
    #[serde(default)]
    pub num_replicas: usize,
    /// Whether the underlying stream should be compressed
    #[serde(default)]
    pub compression: bool,
}

use std::str::FromStr;

/// StorageType represents the type of storage backend to use; it maps the configuration value to the proper 'nats_async' type
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StorageType(pub async_nats::jetstream::stream::StorageType);

/// Default implementation for [`StorageType`]
impl Default for StorageType {
    fn default() -> Self {
        // Use File as default storage type
        Self(async_nats::jetstream::stream::StorageType::File)
    }
}

/// Implementing FromStr for StorageType to allow custom parsing from a string
impl FromStr for StorageType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "file" => Ok(StorageType(
                async_nats::jetstream::stream::StorageType::File,
            )),
            "0" => Ok(StorageType(
                async_nats::jetstream::stream::StorageType::File,
            )),
            "memory" => Ok(StorageType(
                async_nats::jetstream::stream::StorageType::Memory,
            )),
            "1" => Ok(StorageType(
                async_nats::jetstream::stream::StorageType::Memory,
            )),
            _ => Err(()),
        }
    }
}

/// Default implementation for [`StorageConfig`]
impl Default for StorageConfig {
    fn default() -> StorageConfig {
        StorageConfig {
            max_age: core::time::Duration::from_secs(0), // unlimited
            max_bytes: -1,                               // unlimited
            storage_type: StorageType::default(),
            num_replicas: 1,
            compression: false,
        }
    }
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
        if extra.storage_config.is_some() {
            out.storage_config.clone_from(&extra.storage_config);
        }
        if extra.max_write_wait.is_some() {
            out.max_write_wait.clone_from(&extra.max_write_wait);
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
            auth_jwt: None,
            auth_seed: None,
            tls_ca: None,
            tls_ca_file: None,
            storage_config: None,
            max_write_wait: Some(30),
        }
    }
}

impl NatsConnectionConfig {
    /// Construct configuration from a given [`LinkConfig`] or [`HostData`], utilizing both config and secrets provided
    pub fn from_link_config(
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
                warn!("secret value [{CONFIG_NATS_CLIENT_JWT}] is missing, but was found in configuration. Please prefer using secrets for sensitive values.");
            }
            map.insert(CONFIG_NATS_CLIENT_JWT.into(), jwt.to_string());
        }

        if let Some(seed) = secrets
            .get(CONFIG_NATS_CLIENT_SEED)
            .and_then(SecretValue::as_string)
            .or_else(|| config.get(CONFIG_NATS_CLIENT_SEED).map(String::as_str))
        {
            if secrets.get(CONFIG_NATS_CLIENT_SEED).is_none() {
                warn!("secret value [{CONFIG_NATS_CLIENT_SEED}] is missing, but was found configuration. Please prefer using secrets for sensitive values.");
            }
            map.insert(CONFIG_NATS_CLIENT_SEED.into(), seed.to_string());
        }

        if let Some(tls_ca) = secrets
            .get(CONFIG_NATS_TLS_CA)
            .and_then(SecretValue::as_string)
            .or_else(|| config.get(CONFIG_NATS_TLS_CA).map(String::as_str))
        {
            if secrets.get(CONFIG_NATS_TLS_CA).is_none() {
                warn!("secret value [{CONFIG_NATS_TLS_CA}] is missing, but was found configuration. Please prefer using secrets for sensitive values.");
            }
            map.insert(CONFIG_NATS_TLS_CA.into(), tls_ca.to_string());
        } else if let Some(tls_ca_file) = config.get(CONFIG_NATS_TLS_CA_FILE)
        // .or_else(|| config.get(CONFIG_NATS_TLS_CA_FILE).map(String::as_str))
        {
            map.insert(CONFIG_NATS_TLS_CA_FILE.into(), tls_ca_file.to_string());
        }

        // NATS Object Storage Configuration
        let mut storage_config = StorageConfig::default();
        if let Some(max_age) = config.get(CONFIG_NATS_STORAGE_MAX_AGE) {
            storage_config.max_age = core::time::Duration::from_secs(
                max_age.parse().expect("max_age must be a number (seconds)"),
            );
        }
        if let Some(storage_type) = config.get(CONFIG_NATS_STORAGE_TYPE) {
            storage_config.storage_type = storage_type
                .parse::<StorageType>()
                .expect("invalid storage_type");
        }
        if let Some(num_replicas) = config.get(CONFIG_NATS_STORAGE_NUM_REPLICAS) {
            storage_config.num_replicas =
                num_replicas.parse().expect("num_replicas must be a number");
        }
        if let Some(compression) = config.get(CONFIG_NATS_STORAGE_COMPRESSION) {
            storage_config.compression =
                compression.parse().expect("compression must be a boolean");
        }
        map.insert(
            "storage_config".into(),
            serde_json::to_string(&storage_config).expect("failed to serialize storage_config"),
        );

        Self::from_map(&map)
    }

    /// Construct configuration Struct from the passed hostdata config
    pub fn from_map(values: &HashMap<String, String>) -> Result<NatsConnectionConfig> {
        let mut config = NatsConnectionConfig::default();

        if let Some(uri) = values.get(CONFIG_NATS_URI) {
            config.cluster_uri = Some(uri.clone());
        }
        if let Some(domain) = values.get(CONFIG_NATS_JETSTREAM_DOMAIN) {
            config.js_domain = Some(domain.clone());
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
        if config.storage_config.is_none() {
            if let Some(storage_config) = values.get("storage_config") {
                config.storage_config = Some(serde_json::from_str(storage_config)?);
            }
        }
        if let Some(max_write_wait) = values.get(CONFIG_NATS_MAX_WRITE_WAIT) {
            config.max_write_wait = Some(max_write_wait.clone().parse().unwrap());
        }

        Ok(config)
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
    "auth_jwt": "authy",
    "auth_seed": "seedy",
    "max_write_wait": 60
}
"#;

        let config: NatsConnectionConfig = serde_json::from_str(input).unwrap();
        assert_eq!(config.cluster_uri, Some("nats://super-cluster".to_string()));
        assert_eq!(config.js_domain, Some("optional".to_string()));
        assert_eq!(config.auth_jwt.unwrap(), "authy");
        assert_eq!(config.auth_seed.unwrap(), "seedy");
        assert_eq!(config.max_write_wait.unwrap(), 60);
    }

    // Verify that two NatsConnectionConfigs could be merged
    #[test]
    fn test_connectionconfig_merge() {
        let ncc1 = NatsConnectionConfig {
            cluster_uri: Some("old_server".to_string()),
            max_write_wait: Some(45),
            ..Default::default()
        };
        let ncc2 = NatsConnectionConfig {
            cluster_uri: Some("server1".to_string()),
            js_domain: Some("new_domain".to_string()),
            auth_jwt: Some("jawty".to_string()),
            max_write_wait: Some(60),
            ..Default::default()
        };
        let ncc3 = ncc1.merge(&ncc2);
        assert_eq!(ncc3.cluster_uri, ncc2.cluster_uri);
        assert_eq!(ncc3.js_domain, ncc2.js_domain);
        assert_eq!(ncc3.auth_jwt, Some("jawty".to_string()));
        assert_eq!(ncc3.max_write_wait, Some(60));
    }

    // Verify that two configs, which include StorageConfigs could be merged
    #[test]
    fn test_merge_with_storage_config() {
        let ncc1 = NatsConnectionConfig {
            cluster_uri: Some("server1".to_string()),
            js_domain: Some("domain1".to_string()),
            auth_jwt: Some("jwt1".to_string()),
            storage_config: Some(StorageConfig {
                storage_type: StorageType(async_nats::jetstream::stream::StorageType::File),
                compression: true,
                ..Default::default()
            }),
            max_write_wait: Some(45),
            ..Default::default()
        };
        let ncc2 = NatsConnectionConfig {
            cluster_uri: Some("server1".to_string()),
            js_domain: Some("new_domain".to_string()),
            auth_jwt: Some("new_jwt".to_string()),
            storage_config: Some(StorageConfig {
                storage_type: StorageType(async_nats::jetstream::stream::StorageType::Memory),
                compression: false,
                ..Default::default()
            }),
            max_write_wait: Some(60),
            ..Default::default()
        };
        let ncc3 = ncc1.merge(&ncc2);
        assert_eq!(ncc3.cluster_uri, ncc2.cluster_uri);
        assert_eq!(ncc3.js_domain, ncc2.js_domain);
        assert_eq!(ncc3.auth_jwt, ncc2.auth_jwt);
        assert_eq!(ncc3.max_write_wait, ncc2.max_write_wait);
        assert_eq!(
            ncc3.storage_config.clone().unwrap().storage_type,
            ncc2.storage_config.clone().unwrap().storage_type
        );
        assert_eq!(
            ncc3.storage_config.unwrap().compression,
            ncc2.storage_config.unwrap().compression
        );
    }

    // Verify that a NatsConnectionConfig could be constructed from a HashMap
    #[test]
    fn test_from_map_multiple_entries() -> anyhow::Result<()> {
        let mut config = HashMap::new();
        config.insert("tls_ca".to_string(), "rootCA".to_string());
        config.insert("js_domain".to_string(), "optional".to_string());
        config.insert(CONFIG_NATS_CLIENT_JWT.to_string(), "authy".to_string());
        config.insert(CONFIG_NATS_CLIENT_SEED.to_string(), "seedy".to_string());
        config.insert(CONFIG_NATS_MAX_WRITE_WAIT.to_string(), "45".to_string());
        config.insert(CONFIG_NATS_STORAGE_MAX_AGE.to_string(), "3600".to_string());

        let ncc = NatsConnectionConfig::from_link_config(&config, &HashMap::new())?;

        assert_eq!(ncc.tls_ca, Some("rootCA".to_string()));
        assert_eq!(ncc.js_domain, Some("optional".to_string()));
        assert_eq!(ncc.auth_jwt, Some("authy".to_string()));
        assert_eq!(ncc.auth_seed, Some("seedy".to_string()));
        assert_eq!(ncc.max_write_wait, Some(45));

        // Validate StorageConfig and max_age
        if let Some(storage_config) = &ncc.storage_config {
            assert_eq!(storage_config.max_age, std::time::Duration::from_secs(3600));
        }

        Ok(())
    }

    // Verify that a default NatsConnectionConfig will be constructed from an empty HashMap
    #[test]
    fn test_from_map_empty() {
        let config =
            NatsConnectionConfig::from_link_config(&HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(config.cluster_uri, Some(DEFAULT_NATS_URI.into()));
        assert_eq!(config.max_write_wait, Some(30)); // Default value
    }

    // Verify that the NatsConnectionConfig's merge function prioritizes the new values over the old ones
    #[test]
    fn test_merge_non_default_values() {
        let ncc1 = NatsConnectionConfig {
            cluster_uri: Some("old_server".to_string()),
            js_domain: Some("old_domain".to_string()),
            auth_jwt: Some("old_jawty".to_string()),
            ..Default::default()
        };
        let ncc2 = NatsConnectionConfig {
            cluster_uri: Some("server1".to_string()),
            js_domain: Some("new_domain".to_string()),
            auth_jwt: Some("new_jawty".to_string()),
            ..Default::default()
        };
        let ncc3 = ncc1.merge(&ncc2);
        assert_eq!(ncc3.cluster_uri, ncc2.cluster_uri);
        assert_eq!(ncc3.js_domain, ncc2.js_domain);
        assert_eq!(ncc3.auth_jwt, ncc2.auth_jwt);
    }
}
