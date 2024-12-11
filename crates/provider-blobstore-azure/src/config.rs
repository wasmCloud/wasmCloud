//! Configuration for blobstore-azblob capability provider
//!
//! See README.md for configuration options using environment variables, aws credentials files,
//! and EC2 IAM authorizations.
//!

use anyhow::Result;
use serde::Deserialize;
use tracing::warn;

use azure_storage::StorageCredentials;
use wasmcloud_provider_sdk::core::secrets::SecretValue;
use wasmcloud_provider_sdk::LinkConfig;

/// Configuration for connecting to Azblob.
#[derive(Clone, Default, Deserialize)]
pub struct StorageConfig {
    /// STORAGE_ACCOUNT, can be specified from environment
    pub storage_account: String,

    /// STORAGE_ACCESS_KEY, can be in environment
    pub storage_access_key: String,
}

impl StorageConfig {
    /// Build a [`StorageConfig`] from a link configuration
    pub fn from_link_config(
        LinkConfig {
            config, secrets, ..
        }: &LinkConfig,
    ) -> Result<StorageConfig> {
        // To support old workflows, accept but warn when getting the storage access key
        // is not in secrets
        if secrets.get("storage_access_key").is_none() {
            warn!("secret [storage_access_key] was not found, checking for [STORAGE_ACCESS_KEY] in configuration. Please prefer using secrets for sensitive values.");
        }
        match (
            config.get("STORAGE_ACCOUNT"),
            secrets
                .get("storage_access_key")
                .and_then(SecretValue::as_string)
                .or_else(|| config.get("STORAGE_ACCESS_KEY").map(String::as_str)),
        ) {
            (Some(account), Some(access_key)) => Ok(StorageConfig {
                storage_account: account.to_string(),
                storage_access_key: access_key.to_string(),
            }),
            _ => Err(anyhow::anyhow!(
                "STORAGE_ACCOUNT and STORAGE_ACCESS_KEY must be set"
            )),
        }
    }

    /// Build an access key with the stored storage account and access key
    pub fn access_key(self) -> StorageCredentials {
        StorageCredentials::access_key(self.storage_account, self.storage_access_key)
    }
}
