//! Module with structs for use in managing and accessing secrets in a wasmCloud lattice
use std::collections::HashMap;

use secrecy::SecretBox;
use wasmcloud_runtime::capability::secrets::store::SecretValue;

/// A trait for fetching secrets from a secret store. This is used by the host to fetch secrets
/// from a configured secret store.
///
/// By default, this implementation does nothing and returns an empty map. This is useful for
/// testing or when no secret fetching is required.
#[async_trait::async_trait]
pub trait SecretsManager: Send + Sync {
    /// Fetch secrets by name from the secret store. Additional information is provided that can be
    /// sent to the secret store, such as the entity JWT and host JWT, for additional validation.
    async fn fetch_secrets(
        &self,
        _secret_names: Vec<String>,
        _entity_jwt: Option<&String>,
        _host_jwt: &str,
        _application: Option<&String>,
    ) -> anyhow::Result<HashMap<String, SecretBox<SecretValue>>> {
        Ok(HashMap::with_capacity(0))
    }
}

/// A default implementation of the SecretsManager trait that has no secrets.
#[derive(Default)]
pub struct DefaultSecretsManager {}
impl SecretsManager for DefaultSecretsManager {}
