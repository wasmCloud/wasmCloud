//! Data types used when managing credentials on a wasmCloud host or during operation

use anyhow::bail;
use serde::{Deserialize, Serialize};

/// Credentials for a registry that contains artifacts from which
/// WebAssembly components can be extracted (usually a docker image registry)
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RegistryCredential {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// If supplied, token authentication will be used for the registry
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// If supplied, username and password will be used for HTTP Basic authentication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The type of the registry (only "oci" is supported at this time")
    #[serde(rename = "registryType", default = "default_registry_type")]
    pub registry_type: String,
}

fn default_registry_type() -> String {
    "oci".to_string()
}

impl TryFrom<&RegistryCredential> for oci_distribution::secrets::RegistryAuth {
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
            } => Ok(oci_distribution::secrets::RegistryAuth::Basic(
                username.clone(),
                password.clone(),
            )),

            RegistryCredential {
                username: Some(username),
                password: None,
                token: Some(token),
                ..
            } => Ok(oci_distribution::secrets::RegistryAuth::Basic(
                username.clone(),
                token.clone(),
            )),
            _ => bail!("Invalid OCI registry credentials"),
        }
    }
}
