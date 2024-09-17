use std::path::PathBuf;

use anyhow::{Context as _, Result};

/// The type of a registry
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryType {
    /// OCI registry
    #[default]
    Oci,
}

/// The authentication settings for a registry
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryAuth {
    /// HTTP Basic authentication (username and password)
    Basic(String, String),
    /// token authentication
    Token(String),
    /// No authentication
    #[default]
    Anonymous,
}

impl From<(Option<String>, Option<String>)> for RegistryAuth {
    fn from((maybe_username, maybe_password): (Option<String>, Option<String>)) -> Self {
        match (maybe_username, maybe_password) {
            (Some(username), Some(password)) => Self::Basic(username, password),
            _ => Self::Anonymous,
        }
    }
}

#[cfg(feature = "oci")]
impl From<&RegistryAuth> for oci_distribution::secrets::RegistryAuth {
    fn from(auth: &crate::RegistryAuth) -> Self {
        match auth {
            crate::RegistryAuth::Basic(username, password) => {
                Self::Basic(username.clone(), password.clone())
            }
            _ => Self::Anonymous,
        }
    }
}

#[cfg(feature = "oci")]
impl From<RegistryAuth> for oci_distribution::secrets::RegistryAuth {
    fn from(auth: crate::RegistryAuth) -> Self {
        match auth {
            crate::RegistryAuth::Basic(username, password) => Self::Basic(username, password),
            _ => Self::Anonymous,
        }
    }
}

/// Credentials for a registry containing wasmCloud artifacts
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct RegistryConfig {
    /// The type of the registry (only OCI is supported at this time)
    pub(crate) reg_type: RegistryType,
    /// The auth settings for the registry
    pub(crate) auth: RegistryAuth,
    /// Whether or not to allow downloading artifacts with the tag `latest`. Only valid for OCI registries
    pub(crate) allow_latest: bool,
    /// Whether or not to allow downloading artifacts over HTTP
    pub(crate) allow_insecure: bool,
    /// Additional CAs to include in the OCI client configuration
    pub(crate) additional_ca_paths: Vec<PathBuf>,
}

/// Builder for constructing a [`RegistryConfig`]
///
/// While `reg_type` and `auth` are not explicitly required, they must be provided, otherwise
/// building will fail.
#[derive(Debug, Clone, Default)]
#[allow(unused)]
pub struct RegistryConfigBuilder {
    reg_type: Option<RegistryType>,
    auth: Option<RegistryAuth>,
    allow_latest: Option<bool>,
    allow_insecure: Option<bool>,
    additional_ca_paths: Option<Vec<PathBuf>>,
}

impl RegistryConfigBuilder {
    pub fn reg_type(mut self, rt: RegistryType) -> Self {
        self.reg_type = Some(rt);
        self
    }

    pub fn auth(mut self, ra: RegistryAuth) -> Self {
        self.auth = Some(ra);
        self
    }

    pub fn allow_latest(mut self, latest: bool) -> Self {
        self.allow_latest = Some(latest);
        self
    }

    pub fn allow_insecure(mut self, insecure: bool) -> Self {
        self.allow_insecure = Some(insecure);
        self
    }

    pub fn additional_ca_paths(mut self, acp: impl IntoIterator<Item = PathBuf>) -> Self {
        self.additional_ca_paths = Some(acp.into_iter().collect::<Vec<PathBuf>>());
        self
    }

    pub fn build(self) -> Result<RegistryConfig> {
        Ok(RegistryConfig {
            reg_type: self.reg_type.context("missing reg type")?,
            auth: self.auth.context("missing reg auth")?,
            allow_latest: self.allow_insecure.unwrap_or_default(),
            allow_insecure: self.allow_insecure.unwrap_or_default(),
            additional_ca_paths: self.additional_ca_paths.unwrap_or_default(),
        })
    }
}

impl RegistryConfig {
    pub fn builder() -> RegistryConfigBuilder {
        RegistryConfigBuilder::default()
    }

    pub fn reg_type(&self) -> &RegistryType {
        &self.reg_type
    }

    pub fn auth(&self) -> &RegistryAuth {
        &self.auth
    }

    pub fn set_auth(&mut self, value: RegistryAuth) {
        self.auth = value;
    }

    pub fn allow_latest(&self) -> bool {
        self.allow_latest
    }

    pub fn set_allow_latest(&mut self, value: bool) {
        self.allow_latest = value;
    }

    pub fn allow_insecure(&self) -> bool {
        self.allow_insecure
    }

    pub fn set_allow_insecure(&mut self, value: bool) {
        self.allow_insecure = value;
    }

    pub fn additional_ca_paths(&self) -> &Vec<PathBuf> {
        &self.additional_ca_paths
    }

    pub fn set_additional_ca_paths(&mut self, value: Vec<PathBuf>) {
        self.additional_ca_paths = value;
    }
}
