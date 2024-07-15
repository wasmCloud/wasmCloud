use anyhow::ensure;
use async_trait::async_trait;
use nkeys::XKey;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use wascap::jwt::{validate_token, CapabilityProvider, Component, Host};

mod errors;
pub use crate::errors::*;

/// The key of a NATS header containing the wasmCloud host's public xkey used to encrypt a secret request.
/// It is also used to encrypt the response so that only the requestor can decrypt it.
pub const WASMCLOUD_HOST_XKEY: &str = "WasmCloud-Host-Xkey";
pub const RESPONSE_XKEY: &str = "Server-Response-Xkey";

/// The type of secret.
/// This is used to inform wadm or anything else that is consuming the secret about how to
/// deserialize the payload.
pub const SECRET_TYPE: &str = "v1.secret.wasmcloud.dev";

/// The prefix for all secret keys in the config store
pub const SECRET_PREFIX: &str = "SECRET";

/// The request context for retrieving a secret
#[derive(Serialize, Deserialize, Default)]
pub struct Context {
    /// The component or provider's signed JWT.
    pub entity_jwt: String,
    /// The host's signed JWT.
    pub host_jwt: String,
    /// The application the entity belongs to.
    /// TODO: should this also be a JWT, but signed by the host?
    pub application: Option<Application>,
}

/// The application that the entity belongs to.
#[derive(Serialize, Deserialize)]
pub struct Application {
    pub name: String,
}

impl Context {
    /// Validates that the underlying claims embedded in the Context's JWTs are valid.
    pub fn valid_claims(&self) -> Result<(), ContextValidationError> {
        let component_valid = Self::valid_component(&self.entity_jwt);
        let provider_valid = Self::valid_provider(&self.entity_jwt);
        if component_valid.is_err() && provider_valid.is_err() {
            if let Err(e) = component_valid {
                return Err(ContextValidationError::InvalidComponentJWT(e.to_string()));
            } else {
                return Err(ContextValidationError::InvalidProviderJWT(
                    provider_valid.unwrap_err().to_string(),
                ));
            }
        }

        if Self::valid_host(&self.host_jwt).is_err() {
            return Err(ContextValidationError::InvalidHostJWT(
                Self::valid_host(&self.host_jwt).unwrap_err().to_string(),
            ));
        }
        Ok(())
    }

    fn valid_component(token: &str) -> anyhow::Result<()> {
        let v = validate_token::<Component>(token)?;
        ensure!(!v.expired, "token expired at `{}`", v.expires_human);
        ensure!(
            !v.cannot_use_yet,
            "token cannot be used before `{}`",
            v.not_before_human
        );
        ensure!(v.signature_valid, "signature is not valid");
        Ok(())
    }

    fn valid_provider(token: &str) -> anyhow::Result<()> {
        let v = validate_token::<CapabilityProvider>(token)?;
        ensure!(!v.expired, "token expired at `{}`", v.expires_human);
        ensure!(
            !v.cannot_use_yet,
            "token cannot be used before `{}`",
            v.not_before_human
        );
        ensure!(v.signature_valid, "signature is not valid");

        Ok(())
    }

    fn valid_host(token: &str) -> anyhow::Result<()> {
        let v = validate_token::<Host>(token)?;
        ensure!(!v.expired, "token expired at `{}`", v.expires_human);
        ensure!(
            !v.cannot_use_yet,
            "token cannot be used before `{}`",
            v.not_before_human
        );
        ensure!(v.signature_valid, "signature is not valid");
        Ok(())
    }
}

/// The request to retrieve a secret. This includes the name of the secret and the context needed
/// to validate the requestor. The context will be passed to the underlying secrets service in
/// order to make decisions around access.
/// The version field is optional but highly recommended. If it is not provided, the service will
/// default to retrieving the latest version of the secret.
#[derive(Serialize, Deserialize)]
pub struct SecretRequest {
    // The name of the secret
    pub name: String,
    // The version of the secret
    pub version: Option<String>,
    pub context: Context,
}

/// The response to a secret request. The fields are mutually exclusive: either a secret or an
/// error will be set.
#[derive(Serialize, Deserialize, Default)]
pub struct SecretResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<Secret>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<GetSecretError>,
}

/// A secret that can be either a string or binary value.
#[derive(Serialize, Deserialize, Default)]
pub struct Secret {
    pub name: String,
    pub version: String,
    pub string_secret: Option<String>,
    pub binary_secret: Option<Vec<u8>>,
}

/// The representation of a secret reference in the config store.
#[derive(Serialize, Deserialize)]
pub struct SecretConfig {
    pub backend: String,
    pub key: String,
    pub version: Option<String>,

    /// The type of secret.
    /// This is used to inform wadm or anything else that is consuming the secret about how to
    /// deserialize the payload.
    #[serde(rename = "type")]
    pub secret_type_identifier: String,
}

/// Helper function to convert a SecretConfig into a HashMap. This is only intended to be used by
/// wash or anything else that needs to interact directly with the config KV bucket to manipulate
/// secrets.
impl TryInto<HashMap<String, String>> for SecretConfig {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<HashMap<String, String>, Self::Error> {
        let mut map = HashMap::new();
        map.insert("type".to_string(), SECRET_TYPE.to_string());
        map.insert("backend".to_string(), self.backend);
        map.insert("key".to_string(), self.key);
        if let Some(version) = self.version {
            map.insert("version".to_string(), version);
        }
        Ok(map)
    }
}

#[async_trait]
pub trait SecretsServer {
    // Returns the secret value for the given secret name
    async fn get(&self, request: SecretRequest) -> Result<SecretResponse, GetSecretError>;

    // Returns the server's public XKey
    fn server_xkey(&self) -> XKey;
}
