//! Data types used when managing credentials on a wasmCloud host or during operation

use anyhow::bail;
use serde::{Deserialize, Serialize};

/// Credentials for a registry that contains WebAssembly component artifacts.
///
/// While this is usually a docker image registry, other registries may be supported
/// in the future.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct RegistryCredential {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) password: Option<String>,
    /// If supplied, token authentication will be used for the registry
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token: Option<String>,
    /// If supplied, username and password will be used for HTTP Basic authentication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) username: Option<String>,
    /// The type of the registry (only "oci" is supported at this time")
    #[serde(rename = "registryType", default = "default_registry_type")]
    pub(crate) registry_type: String,
}

impl RegistryCredential {
    /// Create a [`RegistryCredential`] from username and password
    #[must_use]
    pub fn from_username_password(username: &str, password: &str, registry_type: &str) -> Self {
        Self {
            username: Some(username.into()),
            password: Some(password.into()),
            token: None,
            registry_type: registry_type.into(),
        }
    }

    /// Create a [`RegistryCredential`] from token
    #[must_use]
    pub fn from_token(token: &str, registry_type: &str) -> Self {
        Self {
            username: None,
            password: None,
            token: Some(token.into()),
            registry_type: registry_type.into(),
        }
    }

    #[must_use]
    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    #[must_use]
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    #[must_use]
    pub fn registry_type(&self) -> &str {
        &self.registry_type
    }
}

/// Helper for creating the default registry type
fn default_registry_type() -> String {
    "oci".to_string()
}

impl TryFrom<&RegistryCredential> for oci_client::secrets::RegistryAuth {
    type Error = anyhow::Error;

    fn try_from(cred: &RegistryCredential) -> Result<Self, Self::Error> {
        if cred.registry_type != "oci" {
            bail!("Only OCI registries are supported at this time");
        }

        match cred {
            RegistryCredential {
                username: Some(username),
                password: Some(password),
                ..
            } => Ok(oci_client::secrets::RegistryAuth::Basic(
                username.clone(),
                password.clone(),
            )),

            RegistryCredential {
                username: Some(username),
                password: None,
                token: Some(token),
                ..
            } => Ok(oci_client::secrets::RegistryAuth::Basic(
                username.clone(),
                token.clone(),
            )),
            _ => bail!("Invalid OCI registry credentials"),
        }
    }
}
