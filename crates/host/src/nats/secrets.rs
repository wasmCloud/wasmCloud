//! Module with structs for use in managing and accessing secrets in a wasmCloud lattice
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, ensure, Context as _};
use async_nats::Client;
use futures::stream;
use futures::stream::{StreamExt, TryStreamExt};
use secrecy::SecretBox;
use tokio::sync::RwLock;
use tracing::instrument;
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_secrets_client::Client as WasmcloudSecretsClient;
use wasmcloud_secrets_types::{Secret as WasmcloudSecret, SecretConfig};

use crate::secrets::SecretsManager;
use crate::store::StoreManager;

/// A manager for fetching secrets from a secret store, caching secrets clients for efficiency.
pub struct NatsSecretsManager {
    config_store: Arc<dyn StoreManager>,
    /// The topic to use for configuring clients to fetch secrets from the secret store.
    secret_store_topic: Option<String>,
    nats_client: Client,
    /// A map of backend names, e.g. nats-kv or vault, to secrets clients, used to cache clients for efficiency.
    backend_clients: Arc<RwLock<HashMap<String, Arc<WasmcloudSecretsClient>>>>,
}

impl NatsSecretsManager {
    /// Create a new secret manager with the given configuration store, secret store topic, and NATS client.
    ///
    /// All secret references will be fetched from this configuration store and the actual secrets will be
    /// fetched by sending requests to the configured topic. If the provided secret_store_topic is None, this manager
    /// will always return an error if [`Self::fetch_secrets`] is called with a list of secrets.
    pub fn new(
        config_store: Arc<dyn StoreManager>,
        secret_store_topic: Option<&String>,
        nats_client: &Client,
    ) -> Self {
        Self {
            config_store,
            secret_store_topic: secret_store_topic.cloned(),
            nats_client: nats_client.clone(),
            backend_clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the secrets client for the provided backend, creating a new client if one does not already exist.
    ///
    /// Returns an error if the secret store topic is not configured, or if the client could not be created.
    async fn get_or_create_secrets_client(
        &self,
        backend: &str,
    ) -> anyhow::Result<Arc<WasmcloudSecretsClient>> {
        let Some(secret_store_topic) = self.secret_store_topic.as_ref() else {
            return Err(anyhow::anyhow!(
                "secret store not configured, could not create secrets client"
            ));
        };

        // If we already have a client for this backend, return it
        // NOTE(brooksmtownsend): This is block scoped to ensure we drop the read lock
        let client = {
            match self.backend_clients.read().await.get(backend) {
                Some(existing) => return Ok(existing.clone()),
                None => Arc::new(
                    WasmcloudSecretsClient::new(
                        backend,
                        secret_store_topic,
                        self.nats_client.clone(),
                    )
                    .await
                    .context("failed to create secrets client")?,
                ),
            }
        };

        self.backend_clients
            .write()
            .await
            .insert(backend.to_string(), client.clone());
        Ok(client)
    }
}

#[async_trait::async_trait]
impl SecretsManager for NatsSecretsManager {
    /// Fetches secret references from the CONFIGDATA bucket by name and then fetches the actual secrets
    /// from the configured secret store. Any error returned from this function should result in a failure
    /// to start a component, start a provider, or establish a link as a missing secret is a critical
    /// error.
    ///
    /// # Arguments
    /// * `secret_names` - A list of secret names to fetch from the secret store
    /// * `entity_jwt` - The JWT of the entity requesting the secrets. Must be provided unless this [`Manager`] is not
    ///  configured with a secret store topic.
    /// * `host_jwt` - The JWT of the host requesting the secrets
    /// * `application` - The name of the application the entity is a part of, if any
    ///
    /// # Returns
    /// A HashMap from secret name to the [`secrecy::Secret`] wrapped [`SecretValue`].
    #[instrument(level = "debug", skip(self, host_jwt))]
    async fn fetch_secrets(
        &self,
        secret_names: Vec<String>,
        entity_jwt: Option<&String>,
        host_jwt: &str,
        application: Option<&String>,
    ) -> anyhow::Result<HashMap<String, SecretBox<SecretValue>>> {
        // If we're not fetching any secrets, return empty map successfully
        if secret_names.is_empty() {
            return Ok(HashMap::with_capacity(0));
        }

        // Attempting to fetch secrets without a secret store topic is always an error
        ensure!(
            self.secret_store_topic.is_some(),
            "secret store not configured, could not fetch secrets"
        );

        // If we don't have an entity JWT, we can't provide its identity to the secrets backend
        let entity_jwt = entity_jwt.context("entity did not have an embedded JWT, required to fetch secrets (was this entity signed during build?)")?;

        let secrets = stream::iter(secret_names.into_iter())
            // Fetch the secret reference from the config store
            .then(|secret_name| async move {
                match self.config_store.get(&secret_name).await {
                    Ok(Some(secret)) => serde_json::from_slice::<SecretConfig>(&secret)
                        .with_context(|| format!("failed to deserialize secret reference from config store, ensure {secret_name} is a secret reference and not configuration")),
                    Ok(None) => bail!(
                        "Secret config {secret_name} not found in config store, could not create secret request"
                    ),
                    Err(e) => bail!(e),
                }
            })
            // Retrieve the actual secret from the secrets backend
            .and_then(|secret_config| async move {
                let secrets_client = self
                    .get_or_create_secrets_client(&secret_config.backend)
                    .await?;
                let secret_name = secret_config.name.clone();
                let request = secret_config.try_into_request(entity_jwt, host_jwt, application).context("failed to create secret request")?;
                secrets_client
                    .get(request, nkeys::XKey::new())
                    .await
                    .map(|secret| (secret_name, secret))
                    .map_err(|e| anyhow::anyhow!(e))
            })
            // Build the map of secrets depending on if the secret is a string or bytes
            .try_fold(HashMap::new(), |mut secrets, (secret_name, secret_result)| async move {
                match secret_result {
                    // NOTE(brooksmtownsend): We create this map using the `secret_name` passed in on from the secret reference
                    // because that's the name that the component/provider will use to look up the secret.
                    WasmcloudSecret {
                        string_secret: Some(string_secret),
                        ..
                    } => secrets.insert(
                        secret_name,
                        SecretBox::new(SecretValue::String(string_secret).into()),
                    ),
                    WasmcloudSecret {
                        binary_secret: Some(binary_secret),
                        ..
                    } => {
                        secrets.insert(
                            secret_name,
                            SecretBox::new(SecretValue::Bytes(binary_secret).into()),
                        )
                    }
                    WasmcloudSecret {
                        string_secret: None,
                        binary_secret: None,
                        ..
                    } => bail!("secret {secret_name} did not contain a value"),
                };
                Ok(secrets)
            })
            .await?;

        Ok(secrets)
    }
}
