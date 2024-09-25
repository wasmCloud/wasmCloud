use anyhow::Result;
use tracing::warn;
use wasmcloud_control_interface::RegistryCredential;
use wasmcloud_core::{RegistryAuth, RegistryConfig, RegistryType};

/// Extension trait to enable converting between registry credentials
pub trait RegistryCredentialExt {
    /// Convert a [`RegistryCredential`] to a [`RegistryConfig`]
    fn into_registry_config(self) -> Result<RegistryConfig>;
}

impl RegistryCredentialExt for RegistryCredential {
    fn into_registry_config(self) -> Result<RegistryConfig> {
        RegistryConfig::builder()
            .reg_type(match self.registry_type() {
                "oci" => RegistryType::Oci,
                registry_type => {
                    warn!(%registry_type, "unknown registry type, defaulting to OCI");
                    RegistryType::Oci
                }
            })
            .auth(match (self.username(), self.password(), self.token()) {
                (Some(username), Some(password), _) => RegistryAuth::Basic(username.into(), password.into()),
                (None, None, Some(token)) => RegistryAuth::Token(token.into()),
                (None, None, None) => RegistryAuth::Anonymous,
                (_, _, _) => {
                    warn!("invalid combination of registry credentials, defaulting to no authentication");
                    RegistryAuth::Anonymous
                }
            })
            .allow_latest(false)
            .allow_insecure(false)
            .additional_ca_paths(Vec::new())
            .build()
    }
}
