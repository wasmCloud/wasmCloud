use std::collections::{hash_map::Entry, HashMap};

use anyhow::Result;
use tracing::{debug, instrument, warn};
use wasmcloud_control_interface::RegistryCredential;
use wasmcloud_core::{RegistryAuth, RegistryConfig, RegistryType};

use crate::OciConfig;

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

#[derive(Debug, Default)]
/// A struct to hold supplemental [RegistryConfig] for the host.
pub struct SupplementalConfig {
    /// A map of registry URLs to [RegistryConfig] to use for it when
    /// fetching images from OCI registries.
    pub registry_config: Option<HashMap<String, RegistryConfig>>,
}

/// A helper function to merge [crate::oci::Config] into the given registry configuration
#[instrument(level = "debug", skip_all)]
pub async fn merge_registry_config(
    registry_config: &mut HashMap<String, RegistryConfig>,
    oci_opts: OciConfig,
) {
    // let mut registry_config = registry_config.write().await;
    let allow_latest = oci_opts.allow_latest;
    let additional_ca_paths = oci_opts.additional_ca_paths;

    // update auth for specific registry, if provided
    if let Some(reg) = oci_opts.oci_registry {
        match registry_config.entry(reg.clone()) {
            Entry::Occupied(_entry) => {
                // note we don't update config here, since the config service should take priority
                warn!(oci_registry_url = %reg, "ignoring OCI registry config, overridden by config service");
            }
            Entry::Vacant(entry) => {
                debug!(oci_registry_url = %reg, "set registry config");
                entry.insert(
                    RegistryConfig::builder()
                        .reg_type(RegistryType::Oci)
                        .auth(RegistryAuth::from((
                            oci_opts.oci_user,
                            oci_opts.oci_password,
                        )))
                        .build()
                        .expect("failed to build registry config"),
                );
            }
        }
    }

    // update or create entry for all registries in allowed_insecure
    oci_opts.allowed_insecure.into_iter().for_each(|reg| {
        match registry_config.entry(reg.clone()) {
            Entry::Occupied(mut entry) => {
                debug!(oci_registry_url = %reg, "set allowed_insecure");
                entry.get_mut().set_allow_insecure(true);
            }
            Entry::Vacant(entry) => {
                debug!(oci_registry_url = %reg, "set allowed_insecure");
                entry.insert(
                    RegistryConfig::builder()
                        .reg_type(RegistryType::Oci)
                        .allow_insecure(true)
                        .build()
                        .expect("failed to build registry config"),
                );
            }
        }
    });

    // update allow_latest for all registries
    registry_config.iter_mut().for_each(|(url, config)| {
        if !additional_ca_paths.is_empty() {
            config.set_additional_ca_paths(additional_ca_paths.clone());
        }
        if allow_latest {
            debug!(oci_registry_url = %url, "set allow_latest");
        }
        config.set_allow_latest(allow_latest);
    });
}
