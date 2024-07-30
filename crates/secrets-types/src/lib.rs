use anyhow::{ensure, Context as _};
use async_trait::async_trait;
use nkeys::XKey;
use serde::{ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use wascap::jwt::{validate_token, CapabilityProvider, Component, Host};

mod errors;
pub use crate::errors::*;

/// The version of the secrets API
pub const SECRET_API_VERSION: &str = "v1alpha1";

/// The key of a NATS header containing the wasmCloud host's public xkey used to encrypt a secret request.
/// It is also used to encrypt the response so that only the requestor can decrypt it.
pub const WASMCLOUD_HOST_XKEY: &str = "WasmCloud-Host-Xkey";
pub const RESPONSE_XKEY: &str = "Server-Response-Xkey";

/// The type of secret.
/// This is used to inform wadm or anything else that is consuming the secret about how to
/// deserialize the payload.
pub const SECRET_TYPE: &str = "secret.wasmcloud.dev/v1alpha1";

/// The type of the properties in the secret policy.
/// This is primarily used to version the policy properties format.
pub const SECRET_POLICY_PROPERTIES_TYPE: &str = "properties.secret.wasmcloud.dev/v1alpha1";

/// The prefix for all secret keys in the config store
pub const SECRET_PREFIX: &str = "SECRET";

/// The request context for retrieving a secret
#[derive(Serialize, Deserialize, Default)]
pub struct Context {
    /// The component or provider's signed JWT.
    pub entity_jwt: String,
    /// The host's signed JWT.
    pub host_jwt: String,
    /// Information about the application that the entity belongs to.
    pub application: Application,
}

/// The application that the entity belongs to.
#[derive(Serialize, Deserialize, Default)]
pub struct Application {
    /// The name of the application.
    #[serde(default)]
    pub name: Option<String>,

    /// The policy used define the application's access to secrets.
    /// This meant to be a JSON string that can be deserialized by a secrets backend
    /// implementation.
    #[serde(default)]
    pub policy: String,
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
    /// An identifier of the secret as stored in the secret store.
    ///
    /// This can be a key, path, or any other identifier that the secret store uses to
    /// retrieve a secret.
    pub key: String,
    pub field: Option<String>,
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
    pub version: String,
    pub string_secret: Option<String>,
    pub binary_secret: Option<Vec<u8>>,
}

/// The representation of a secret reference in the config store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretConfig {
    /// The name of the secret when referred to by a component or provider.
    pub name: String,
    /// The backend to use for retrieving the secret.
    pub backend: String,
    /// The key to use for retrieving the secret from the backend.
    pub key: String,
    /// The field to retrieve from the secret. If not supplied, the entire secret will be returned.
    pub field: Option<String>,
    /// The version of the secret to retrieve. If not supplied, the latest version will be used.
    pub version: Option<String>,
    /// The policy that defines configuration options for the backend. This is a serialized
    /// JSON object that will be passed to the backend as a string for policy evaluation.
    pub policy: Policy,

    // NOTE: Should be serialized/deserialized as "type" in JSON
    /// The type of secret.
    /// This is used to inform wadm or anything else that is consuming the secret about how to
    /// deserialize the payload.
    pub secret_type: String,
}

impl SecretConfig {
    pub fn new(
        name: String,
        backend: String,
        key: String,
        field: Option<String>,
        version: Option<String>,
        policy_properties: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            name,
            backend,
            key,
            field,
            version,
            policy: Policy::new(policy_properties),
            secret_type: SECRET_TYPE.to_string(),
        }
    }

    /// Given an entity JWT, host JWT, and optional application name, convert this SecretConfig
    /// into a SecretRequest that can be used to fetch the secret from a secrets backend.
    ///
    /// This is not a true [`TryInto`] implementation as we need additional information to create
    /// the [`SecretRequest`]. This returns an error if the policy field cannot be serialized to a JSON
    /// string.
    pub fn try_into_request(
        self,
        entity_jwt: &str,
        host_jwt: &str,
        application_name: Option<&String>,
    ) -> Result<SecretRequest, anyhow::Error> {
        Ok(SecretRequest {
            key: self.key,
            field: self.field,
            version: self.version,
            context: Context {
                entity_jwt: entity_jwt.to_string(),
                host_jwt: host_jwt.to_string(),
                application: Application {
                    name: application_name.cloned(),
                    policy: serde_json::to_string(&self.policy)
                        .context("failed to serialize secret policy as string")?,
                },
            },
        })
    }
}

/// Helper function to convert a SecretConfig into a HashMap. This is only intended to be used by
/// wash or anything else that needs to interact directly with the config KV bucket to manipulate
/// secrets.
impl TryInto<HashMap<String, String>> for SecretConfig {
    type Error = anyhow::Error;

    /// Convert this SecretConfig into a HashMap of the form:
    /// ```json
    /// {
    ///   "name": "secret-name",
    ///   "type": "secret.wasmcloud.dev/v1alpha1",
    ///   "backend": "baobun",
    ///   "key": "/path/to/secret",
    ///   "version": "vX.Y.Z",
    ///   "policy": "{\"type\":\"properties.secret.wasmcloud.dev/v1alpha1\",\"properties\":{\"key\":\"value\"}}"
    /// }
    /// ```
    fn try_into(self) -> Result<HashMap<String, String>, Self::Error> {
        let mut map = HashMap::from([
            ("name".into(), self.name),
            ("type".into(), self.secret_type),
            ("backend".into(), self.backend),
            ("key".into(), self.key),
        ]);
        if let Some(field) = self.field {
            map.insert("field".to_string(), field);
        }
        if let Some(version) = self.version {
            map.insert("version".to_string(), version);
        }

        map.insert(
            "policy".to_string(),
            serde_json::to_string(&self.policy).context("failed to serialize policy string")?,
        );
        Ok(map)
    }
}

// We need full impls of serialize and deserialize because we have to handle the custom error case
// when serializing the policy to JSON
impl Serialize for SecretConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.version.is_some() { 6 } else { 5 };
        let mut state = serializer.serialize_struct("SecretReference", field_count)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("backend", &self.backend)?;
        state.serialize_field("key", &self.key)?;
        if let Some(v) = self.version.as_ref() {
            state.serialize_field("version", v)?;
        }

        // Serialize policy to JSON string
        let policy_json = serde_json::to_string(&self.policy).map_err(serde::ser::Error::custom)?;
        state.serialize_field("policy", &policy_json)?;
        state.serialize_field("type", &self.secret_type)?;

        state.end()
    }
}

impl<'de> Deserialize<'de> for SecretConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            name: String,
            backend: String,
            key: String,
            field: Option<String>,
            version: Option<String>,
            policy: String,
            #[serde(rename = "type")]
            ty: String,
        }

        let helper = Helper::deserialize(deserializer)?;

        // Deserialize policy from JSON string
        let policy: Policy =
            serde_json::from_str(&helper.policy).map_err(serde::de::Error::custom)?;

        Ok(SecretConfig {
            name: helper.name,
            backend: helper.backend,
            key: helper.key,
            field: helper.field,
            version: helper.version,
            policy,
            secret_type: helper.ty,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Policy {
    #[serde(rename = "type")]
    policy_type: String,
    properties: HashMap<String, serde_json::Value>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            policy_type: SECRET_POLICY_PROPERTIES_TYPE.to_string(),
            properties: Default::default(),
        }
    }
}

impl Policy {
    /// Returns a new policy with the specified properties
    pub fn new(properties: HashMap<String, serde_json::Value>) -> Self {
        Self {
            properties,
            ..Default::default()
        }
    }
}

#[async_trait]
pub trait SecretsServer {
    // Returns the secret value for the given secret name
    async fn get(&self, request: SecretRequest) -> Result<SecretResponse, GetSecretError>;

    // Returns the server's public XKey
    fn server_xkey(&self) -> XKey;
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;
    #[test]
    fn test_secret_config_hashmap_try_into() {
        let properties = HashMap::from([(
            String::from("key"),
            serde_json::Value::String("value".to_string()),
        )]);
        let secret_config = crate::SecretConfig::new(
            "name".to_string(),
            "backend".to_string(),
            "key".to_string(),
            Some("field".to_string()),
            Some("version".to_string()),
            properties,
        );

        let map: HashMap<String, String> = secret_config
            .clone()
            .try_into()
            .expect("should be able to convert to hashmap");

        assert_eq!(map.get("name"), Some(&secret_config.name));
        assert_eq!(map.get("type"), Some(&secret_config.secret_type));
        assert_eq!(map.get("backend"), Some(&secret_config.backend));
        assert_eq!(map.get("key"), Some(&secret_config.key));
        assert_eq!(map.get("field"), secret_config.field.as_ref());
        assert_eq!(map.get("version"), secret_config.version.as_ref());
        assert_eq!(
            map.get("policy"),
            Some(
                &serde_json::to_string(&secret_config.policy)
                    .expect("should be able to serialize policy")
            )
        );
    }
}
