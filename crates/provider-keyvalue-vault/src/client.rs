//! Hashicorp vault client
//!
use core::time::Duration;

use std::string::ToString;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};
use vaultrs::api::kv2::responses::SecretVersionMetadata;
use vaultrs::client::{Client as _, VaultClient, VaultClientSettings};

use wasmcloud_provider_wit_bindgen::deps::serde::{de::DeserializeOwned, Serialize};

use crate::config::Config;
use crate::error::VaultError;

/// Vault HTTP api version. As of Vault 1.9.x (Feb 2022), all http api calls use version 1
const API_VERSION: u8 = 1;

/// Default TTL for tokens used by this provider. Defaults to 72 hours.
pub const TOKEN_INCREMENT_TTL: &str = "72h";
pub const TOKEN_REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60 * 12); // 12 hours

/// Vault client connection information.
#[derive(Clone)]
pub struct Client {
    inner: Arc<vaultrs::client::VaultClient>,
    namespace: String,
    token_increment_ttl: String,
    token_refresh_interval: Duration,
    renew_task: Arc<Mutex<Option<JoinHandle<()>>>>,
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
                identity: None,
            })?),
            namespace: config.mount,
            token_increment_ttl: config
                .token_increment_ttl
                .unwrap_or(TOKEN_INCREMENT_TTL.into()),
            token_refresh_interval: config
                .token_refresh_interval
                .unwrap_or(TOKEN_REFRESH_INTERVAL),
            renew_task: Arc::default(),
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

    /// Sets up a background task to renew the token at the configured interval. This function
    /// attempts to lock the renew_task mutex and will deadlock if called without first ensuring
    /// the lock is available.
    pub async fn set_renewal(&self) {
        let mut renew_task = self.renew_task.lock().await;
        if let Some(handle) = renew_task.take() {
            handle.abort();
        }
        let client = self.inner.clone();
        let interval = self.token_refresh_interval;
        let ttl = self.token_increment_ttl.clone();

        *renew_task = Some(tokio::spawn(async move {
            let mut next_interval = tokio::time::interval(interval);
            loop {
                next_interval.tick().await;
                // NOTE(brooksmtownsend): Errors are appropriately logged in the function
                let _ = renew_self(&client, ttl.as_str()).await;
            }
        }));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // NOTE(brooksmtownsend): We're trying to lock here so we don't deadlock on dropping.
        if let Ok(mut renew_task) = self.renew_task.try_lock() {
            if let Some(handle) = renew_task.take() {
                handle.abort();
            }
        }
    }
}

/// Helper function to renew a client's token, incrementing the validity by `increment`
async fn renew_self(client: &VaultClient, increment: &str) -> Result<(), VaultError> {
    debug!("renewing token");
    client.renew(Some(increment)).await.map_err(|e| {
        error!("error renewing self token: {}", e);
        VaultError::from(e)
    })?;

    let info = client.lookup().await.map_err(|e| {
        error!("error looking up self token: {}", e);
        VaultError::from(e)
    })?;

    let expire_time = info.expire_time.unwrap_or_else(|| "None".to_string());
    info!(%expire_time, accessor = %info.accessor, "renewed token");
    Ok(())
}
