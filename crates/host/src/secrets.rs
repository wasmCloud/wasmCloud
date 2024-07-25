//! Module with structs for use in managing and accessing secrets in a wasmCloud lattice
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, ensure, Context as _};
use async_nats::{jetstream::kv::Store, Client};
use futures::stream;
use futures::stream::{StreamExt, TryStreamExt};
use secrecy::Secret;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::instrument;
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_secrets_client::Client as WasmcloudSecretsClient;
use wasmcloud_secrets_types::{Application, Context, Secret as WasmcloudSecret, SecretRequest};

/// The prefix in the CONFIGDATA bucket for secret references
pub const SECRET_PREFIX: &str = "SECRET_";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub(crate) struct SecretReference {
    /// The backend to use for retrieving the secret.
    pub backend: String,
    /// The key to use for retrieving the secret from the backend.
    pub key: String,
    /// The version of the secret to retrieve. If not supplied, the latest version will be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The policy that defines configuration options for the backend. This is a serialized
    /// JSON object that will be passed to the backend for policy evaluation.
    pub policy: String,
}

#[derive(Debug)]
/// A manager for fetching secrets from a secret store, caching secrets clients for efficiency.
pub struct Manager {
    config_store: Store,
    /// The topic to use for configuring clients to fetch secrets from the secret store.
    secret_store_topic: Option<String>,
    nats_client: Client,
    /// A map of backend names, e.g. nats-kv or vault, to secrets clients, used to cache clients for efficiency.
    backend_clients: Arc<RwLock<HashMap<String, Arc<WasmcloudSecretsClient>>>>,
}

impl Manager {
    /// Create a new secret manager with the given configuration store, secret store topic, and NATS client.
    ///
    /// All secret references will be fetched from this configuration store and the actual secrets will be
    /// fetched by sending requests to the configured topic. If the provided secret_store_topic is None, this manager
    /// will always return an error if [`Self::fetch_secrets`] is called with a list of secrets.
    pub fn new(
        config_store: &Store,
        secret_store_topic: Option<&String>,
        nats_client: &Client,
    ) -> Self {
        Self {
            config_store: config_store.clone(),
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
    #[instrument(level = "debug", skip(host_jwt))]
    pub async fn fetch_secrets(
        &self,
        secret_names: Vec<String>,
        entity_jwt: Option<&String>,
        host_jwt: &str,
        application: Option<&String>,
    ) -> anyhow::Result<HashMap<String, Secret<SecretValue>>> {
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
                    Ok(Some(secret)) => serde_json::from_slice::<SecretReference>(&secret)
                        .with_context(|| format!("failed to deserialize secret reference from config store, ensure {secret_name} is a secret reference and not configuration"))
                        .map(|secret_ref| (secret_name.trim_start_matches(SECRET_PREFIX).to_string(), secret_ref)),
                    Ok(None) => bail!(
                        "Secret reference {secret_name} not found in config store"
                    ),
                    Err(e) => bail!(e),
                }
            })
            // Retrieve the actual secret from the secrets backend
            .and_then(|(secret_name, secret_ref)| async move {
                let secrets_client = self
                    .get_or_create_secrets_client(&secret_ref.backend)
                    .await?;
                let request = SecretRequest {
                    name: secret_ref.key,
                    version: secret_ref.version,
                    context: Context {
                        entity_jwt: entity_jwt.to_string(),
                        host_jwt: host_jwt.to_string(),
                        application: Application {
                            // We pass an empty string if the entity doesn't belong to an application
                            name: application.cloned(),
                            policy: secret_ref.policy,
                        },
                    },
                };
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
                        Secret::new(SecretValue::String(string_secret)),
                    ),
                    WasmcloudSecret {
                        binary_secret: Some(binary_secret),
                        ..
                    } => {
                        secrets.insert(secret_name, Secret::new(SecretValue::Bytes(binary_secret)))
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
