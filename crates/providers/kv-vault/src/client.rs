//! Hashicorp vault client
//!
use std::{string::ToString, sync::Arc};

use serde::{de::DeserializeOwned, Serialize};
use vaultrs::api::kv2::responses::SecretVersionMetadata;
use vaultrs::client::{VaultClient, VaultClientSettings};

use crate::{config::Config, error::VaultError};

/// Vault HTTP api version. As of Vault 1.9.x (Feb 2022), all http api calls use version 1
const API_VERSION: u8 = 1;

/// Vault client connection information.
#[derive(Clone)]
pub struct Client {
    inner: Arc<vaultrs::client::VaultClient>,
    namespace: String,
}

impl Client {
    /// Creates a new Vault client. See [config](./config.rs) for explanation of parameters.
    ///
    /// Note that this constructor does not attempt to connect to the vault server,
    /// so the vault server does not need to be running at the time a LinkDefinition to this provider is created.
    pub fn new(config: Config) -> Result<Self, VaultError> {
        Ok(Client {
            inner: Arc::new(VaultClient::new(VaultClientSettings {
                token: config.token,
                address: config.addr,
                ca_certs: config.certs,
                verify: false,
                version: API_VERSION,
                wrapping: false,
                timeout: None,
                namespace: None,
            })?),
            namespace: config.mount,
        })
    }

    /// Reads value of secret using namespace and key path
    pub async fn read_secret<D: DeserializeOwned>(&self, path: &str) -> Result<D, VaultError> {
        match vaultrs::kv2::read(self.inner.as_ref(), &self.namespace, path).await {
            Err(vaultrs::error::ClientError::APIError {
                code: 404,
                errors: _,
            }) => Err(VaultError::NotFound {
                namespace: self.namespace.clone(),
                path: path.to_string(),
            }),
            Err(e) => Err(e.into()),
            Ok(val) => Ok(val),
        }
    }

    /// Writes value of secret using namespace and key path
    pub async fn write_secret<T: Serialize>(
        &self,
        path: &str,
        data: &T,
    ) -> Result<SecretVersionMetadata, VaultError> {
        vaultrs::kv2::set(self.inner.as_ref(), &self.namespace, path, data)
            .await
            .map_err(VaultError::from)
    }

    /// Deletes the latest version of the secret. Note that if versions are in use, only the latest is deleted
    /// Returns Ok if the key was deleted, or Err for any other error including key not found
    pub async fn delete_latest(&self, path: impl AsRef<str>) -> Result<(), VaultError> {
        let path = path.as_ref();
        vaultrs::kv2::delete_latest(self.inner.as_ref(), &self.namespace, path)
            .await
            .map_err(VaultError::from)
    }

    /// Lists keys at the path
    pub async fn list_secrets(&self, path: &str) -> Result<Vec<String>, VaultError> {
        match vaultrs::kv2::list(self.inner.as_ref(), &self.namespace, path).await {
            Err(vaultrs::error::ClientError::APIError {
                code: 404,
                errors: _,
            }) => Err(VaultError::NotFound {
                namespace: self.namespace.clone(),
                path: path.to_string(),
            }),
            Err(e) => Err(e.into()),
            Ok(secret_list) => Ok(secret_list),
        }
    }
}
