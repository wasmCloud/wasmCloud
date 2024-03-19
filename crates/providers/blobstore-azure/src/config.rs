//! Configuration for blobstore-azblob capability provider
//!
//! See README.md for configuration options using environment variables, aws credentials files,
//! and EC2 IAM authorizations.
//!
use std::collections::HashMap;

use anyhow::Result;
use serde::Deserialize;

use azure_storage::StorageCredentials;

/// Configuration for connecting to Azblob.
#[derive(Clone, Default, Deserialize)]
pub struct StorageConfig {
    /// STORAGE_ACCOUNT, can be specified from environment
    pub storage_account: String,

    /// STORAGE_ACCESS_KEY, can be in environment
    pub storage_access_key: String,
}

impl StorageConfig {
    pub fn from_values(values: &HashMap<String, String>) -> Result<StorageConfig> {
        match (
            values.get("STORAGE_ACCOUNT"),
            values.get("STORAGE_ACCESS_KEY"),
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

    pub fn configure_az(self) -> StorageCredentials {
        StorageCredentials::access_key(self.storage_account, self.storage_access_key)
    }
}
