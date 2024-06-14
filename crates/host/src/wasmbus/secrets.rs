//! Module with structs for use in managing and accessing secrets in a wasmCloud lattice
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use async_nats::{jetstream::kv::Store, Client};
use secrecy::Secret;
use serde::{Deserialize, Serialize};
use tracing::{error, instrument};
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_secrets_types::{Context, SecretRequest};

// NOTE: Copied from wadm, we should have this in secret-types maybe
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct SecretProperty {
    /// The name of the secret. This is used by a reference by the component or capability to
    /// get the secret value as a resource.
    pub name: String,
    /// The source of the secret. This indicates how to retrieve the secret value from a secrets
    /// backend and which backend to actually query.
    pub source: SecretSourceProperty,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct SecretSourceProperty {
    /// The backend to use for retrieving the secret.
    pub backend: String,
    /// The key to use for retrieving the secret from the backend.
    pub key: String,
    /// The version of the secret to retrieve. If not supplied, the latest version will be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Fetches secret references from the CONFIGDATA bucket by name and then fetches the actual secrets
/// from the configured secret store. Any error returned from this function should result in a failure
/// to start a component, start a provider, or establish a link as a missing secret is a critical
/// error.
///
/// TODO(#2344): Consider passing in fewer args
///
/// # Arguments
/// TODO
///
/// # Returns
/// TODO
///
/// TODO(#2344): consider if we need to skip JWTs
#[instrument(level = "debug", skip(config_store, secret_store_topic, nats_client))]
pub async fn fetch_secrets(
    config_store: &Store,
    secret_names: Vec<String>,
    secret_store_topic: Option<&String>,
    nats_client: &Client,
    entity_jwt: &str,
    host_jwt: &str,
    application: Option<&String>,
) -> anyhow::Result<HashMap<String, Secret<SecretValue>>> {
    // If no secret store topic is provided, we don't fetch secrets
    let Some(topic) = secret_store_topic else {
        return Ok(HashMap::with_capacity(0));
    };

    // TODO(#2344): All of this can likely be a single async filter/map/unwrap/whatever to fetch secret refs
    let futs = secret_names.iter().map(|secret_name| async move {
        (secret_name.to_string(), config_store.get(secret_name).await)
    });
    let secret_refs_opts = futures::future::join_all(futs).await;

    let mut backends = HashSet::new();
    let mut secret_refs = vec![];
    for (_name, proppy) in secret_refs_opts {
        match proppy {
            Ok(Some(bytes)) => {
                let source: SecretSourceProperty = serde_json::from_slice(&bytes)?;
                if !backends.contains(&source.backend) {
                    backends.insert(source.backend.clone());
                }
                // TODO(#2344): I think the NATS impl is actually wrong here, the name != the key?
                secret_refs.push(SecretProperty {
                    name: source.key.to_string(),
                    source,
                });
            }
            Ok(None) => error!("SECRET DID NORT EXIST"),
            Err(e) => error!(?e, "ERROR FETCHING SECRET"),
        }
    }
    let mut secrets_backends = HashMap::new();
    // TODO(#2344): We should keep this cached per backend at the host level rather than construct here
    for backend in backends {
        secrets_backends.insert(
            backend.clone(),
            Arc::new(
                wasmcloud_secrets_client::Client::new(&backend, topic, nats_client.clone())
                    .await
                    // TODO(#2344): secret client errors need to be a better error
                    .map_err(|e| {
                        error!(?e, "Failed to create secret client");
                        anyhow::anyhow!("Failed to create secret client")
                    })?,
            ),
        );
    }
    let futs = secret_refs.iter().map(|secret_ref| {
        let secret_client = secrets_backends
            .get(&secret_ref.source.backend)
            // TODO(#2344): This should bubble up an error but it should exist
            .expect("should be here")
            .clone();
        async move {
            let request = SecretRequest {
                name: secret_ref.name.clone(),
                version: secret_ref.source.version.clone(),
                context: Context {
                    entity_jwt: entity_jwt.to_string(),
                    // TODO(#2344): It's convenient, but we should consider minting this JWT each
                    // request
                    host_jwt: host_jwt.to_string(),
                    application: application
                        .cloned()
                        .map(|name| wasmcloud_secrets_types::Application { name }),
                },
            };
            secret_client.get(request, nkeys::XKey::new()).await
        }
    });

    let secret_results = futures::future::join_all(futs).await;

    let mut secrets = HashMap::new();
    for secret_result in secret_results {
        match secret_result {
            Ok(secret) if secret.string_secret.is_some() => {
                secrets.insert(
                    secret.name.clone(),
                    Secret::new(SecretValue::String(
                        secret
                            .string_secret
                            .expect("secret string did exist, this is programmer error"),
                    )),
                );
            }
            Ok(secret) if secret.binary_secret.is_some() => {
                secrets.insert(
                    secret.name.clone(),
                    Secret::new(SecretValue::Bytes(
                        secret
                            .binary_secret
                            .expect("secret binary did exist, this is programmer error"),
                    )),
                );
            }
            // TODO(#2344): What exactly do i do here
            Ok(s) => error!(s.name, "Secret did not contain a value"),
            Err(e) => {
                error!(?e, "Failed to fetch secret");
            }
        }
    }

    Ok(secrets)
}
