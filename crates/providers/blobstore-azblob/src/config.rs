//! Configuration for blobstore-azblob capability provider
//!
//! See README.md for configuration options using environment variables, aws credentials files,
//! and EC2 IAM authorizations.
//!
use std::collections::HashMap;
use std::env;

use anyhow::{Context, Result};
use wasmcloud_provider_wit_bindgen::deps::{serde::Deserialize, serde_json};
use base64::Engine;

use azure_storage::StorageCredentials;

/// Configuration for connecting to Azblob.
#[derive(Clone, Default, Deserialize)]
#[serde(crate = "wasmcloud_provider_wit_bindgen::deps::serde")]
pub struct StorageConfig {
    /// STORAGE_ACCOUNT, can be specified from environment
    pub storage_account: String,

    /// STORAGE_ACCESS_KEY, can be in environment
    pub storage_access_key: String,
}


impl StorageConfig {
    pub fn from_values(values: &HashMap<String, String>) -> Result<StorageConfig> {
        let mut config = if let Some(config_b64) = values.get("config_b64") {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(config_b64.as_bytes())
                .context("invalid base64 encoding")?;
            serde_json::from_slice::<StorageConfig>(&bytes).context("corrupt config_b64")?
        } else if let Some(config) = values.get("config_json") {
            serde_json::from_str::<StorageConfig>(config).context("corrupt config_json")?
        } else {
            StorageConfig::default()
        };

        if let Some(env_file) = values.get("env") {
            let data = std::fs::read_to_string(env_file)
                .with_context(|| format!("reading env file '{env_file}'"))?;
            simple_env_load::parse_and_set(&data, |k, v| std::env::set_var(k, v));
        }
        
        if let Ok(account) = env::var("STORAGE_ACCOUNT") {
            config.storage_account = account;
        }

        if let Ok(access_key) = env::var("STORAGE_ACCESS_KEY") {
            config.storage_access_key = access_key;
        }

        Ok(config)
    }

    pub fn configure_az(self) -> StorageCredentials {
        StorageCredentials::access_key(self.storage_account, self.storage_access_key)
    }
}