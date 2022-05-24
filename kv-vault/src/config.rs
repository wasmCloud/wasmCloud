//! Configuration for kv-vault capability provider
//!
use std::{collections::HashMap, env};
use url::Url;
use wasmbus_rpc::error::{RpcError, RpcResult};

const DEFAULT_VAULT_ADDR: &str = "http://127.0.0.1:8200";

/// KV-Vault configuration
#[derive(Clone, Debug)]
pub struct Config {
    /// Token for connecting to vault, can be set in environment with VAULT_TOKEN.
    /// Required
    pub token: String,
    /// Url for connecting to vault, can be set in environment with VAULT_ADDR.
    /// Defaults to 'http://127.0.0.1:8200'
    pub addr: Url,
    /// Vault mount point, can be set with in environment with VAULT_MOUNT.
    /// Efaults to "secret/"
    pub mount: String,
    /// certificate files - path to CA certificate file(s). Setting this enables TLS
    /// The linkdef value `certs` and the environment variable `VAULT_CERTS`
    /// are parsed as a comma-separated string of file paths to generate this list.
    pub certs: Vec<String>,
}

impl Default for Config {
    /// default constructor - Gets all values from environment & defaults
    fn default() -> Self {
        Self::from_values(&HashMap::new()).unwrap()
    }
}

impl Config {
    /// initialize from linkdef values, environment, and defaults
    pub fn from_values(values: &HashMap<String, String>) -> RpcResult<Config> {
        let config = Config {
            addr: env::var("VAULT_ADDR")
                .ok()
                .or_else(|| values.get("addr").cloned())
                .or_else(|| values.get("ADDR").cloned())
                .unwrap_or_else(|| DEFAULT_VAULT_ADDR.to_string())
                .parse()
                .unwrap_or_else(|_| {
                    eprintln!(
                        "Could not parse VAULT_ADDR as Url, using default of {}",
                        DEFAULT_VAULT_ADDR
                    );
                    DEFAULT_VAULT_ADDR.parse().unwrap()
                }),
            token: env::var("VAULT_TOKEN")
                .ok()
                .or_else(|| values.get("token").cloned())
                .or_else(|| values.get("TOKEN").cloned())
                .ok_or_else(|| {
                    RpcError::ProviderInit("missing setting for 'token' or VAULT_TOKEN".to_string())
                })?,
            mount: env::var("VAULT_MOUNT")
                .ok()
                .or_else(|| values.get("mount").cloned())
                .or_else(|| values.get("mount").cloned())
                .unwrap_or_else(|| "secret".to_string()),
            certs: match env::var("VAULT_CERTS")
                .ok()
                .or_else(|| values.get("certs").cloned())
                .or_else(|| values.get("CERTS").cloned())
            {
                Some(certs) => certs.split(',').map(|s| s.trim().to_string()).collect(),
                _ => Vec::new(),
            },
        };
        Ok(config)
    }
}
