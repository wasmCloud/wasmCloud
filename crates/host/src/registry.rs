use tracing::warn;

/// Credentials for a registry containing wasmCloud artifacts
#[derive(Debug, Default)]
pub struct Config {
    /// The type of the registry (only OCI is supported at this time)
    pub reg_type: Type,
    /// The auth settings for the registry
    pub auth: Auth,
    /// Whether or not to allow downloading artifacts with the tag `latest`. Only valid for OCI registries
    pub allow_latest: bool,
    /// Whether or not to allow downloading artifacts over HTTP
    pub allow_insecure: bool,
}

/// The type of a registry
#[derive(Debug, Default)]
pub enum Type {
    /// OCI registry
    #[default]
    Oci,
}

/// The authentication settings for a registry
#[derive(Debug, Default)]
pub enum Auth {
    /// HTTP Basic authentication (username and password)
    Basic(String, String),
    /// token authentication
    Token(String),
    /// No authentication
    #[default]
    Anonymous,
}

impl From<wasmcloud_control_interface::RegistryCredential> for Config {
    fn from(creds: wasmcloud_control_interface::RegistryCredential) -> Self {
        Self {
            reg_type: match creds.registry_type.as_str() {
                "oci" => Type::Oci,
                registry_type => {
                    warn!(%registry_type, "unknown registry type, defaulting to OCI");
                    Type::Oci
                }
            },
            auth: match (creds.username, creds.password, creds.token) {
                (Some(username), Some(password), _) => Auth::Basic(username, password),
                (None, None, Some(token)) => Auth::Token(token),
                (None, None, None) => Auth::Anonymous,
                (_, _, _) => {
                    warn!("invalid combination of registry credentials, defaulting to no authentication");
                    Auth::Anonymous
                }
            },
            allow_latest: false,
            allow_insecure: false,
        }
    }
}

impl From<(Option<String>, Option<String>)> for Auth {
    fn from((maybe_username, maybe_password): (Option<String>, Option<String>)) -> Self {
        match (maybe_username, maybe_password) {
            (Some(username), Some(password)) => Self::Basic(username, password),
            _ => Self::Anonymous,
        }
    }
}
