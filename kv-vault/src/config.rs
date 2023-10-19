//! Configuration for kv-vault capability provider
//!
use crate::client::TOKEN_REFRESH_INTERVAL;
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

    /// Renewal TTL for tokens used by this provider. Defaults to 72 hours.
    pub token_increment_ttl: Option<String>,

    /// Refresh interval for tokens used by this provider. Defaults to 12 hours.
    pub token_refresh_interval: Option<std::time::Duration>,
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
        // load environment variables from file
        if let Some(env_file) = values.get("env").or_else(|| values.get("ENV")) {
            eprintln!("file try read env from file: {}", env_file);
            let data = std::fs::read_to_string(env_file).map_err(|e| {
                RpcError::ProviderInit(format!("reading env file '{}': {}", env_file, e))
            })?;
            simple_env_load::parse_and_set(&data, |k, v| std::env::set_var(k, v));
        }
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
                .or_else(|| values.get("MOUNT").cloned())
                .unwrap_or_else(|| "secret".to_string()),
            certs: match env::var("VAULT_CERTS")
                .ok()
                .or_else(|| values.get("certs").cloned())
                .or_else(|| values.get("CERTS").cloned())
            {
                Some(certs) => certs.split(',').map(|s| s.trim().to_string()).collect(),
                _ => Vec::new(),
            },
            token_increment_ttl: env::var("VAULT_TOKEN_INCREMENT_TTL")
                .ok()
                .or_else(|| values.get("token_increment_ttl").cloned())
                .or_else(|| values.get("TOKEN_INCREMENT_TTL").cloned()),
            token_refresh_interval: match env::var("VAULT_TOKEN_REFRESH_INTERVAL")
                .ok()
                .or_else(|| values.get("token_refresh_interval").cloned())
                .or_else(|| values.get("TOKEN_REFRESH_INTERVAL").cloned())
            {
                Some(val) => {
                    let secs = val.parse::<u64>().unwrap_or_else(|_| {
                        eprintln!(
                            "Could not parse VAULT_TOKEN_REFRESH_INTERVAL as u64, using default of {}",
                            TOKEN_REFRESH_INTERVAL.as_secs()
                        );
                        TOKEN_REFRESH_INTERVAL.as_secs()
                    });
                    Some(std::time::Duration::from_secs(secs))
                }
                _ => None,
            },
        };
        Ok(config)
    }
}
