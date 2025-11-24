use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::warn;
use wasmcloud_provider_sdk::{core::secrets::SecretValue, types::InterfaceConfig};

const DEFAULT_CTL_HOST: &str = "0.0.0.0";
const DEFAULT_CTL_PORT: u16 = 4222;
const DEFAULT_LATTICE: &str = "default";

// Configuration keys
const CONFIG_LATTICE: &str = "lattice";
const CONFIG_APP_NAME: &str = "app_name";
const CONFIG_CTL_HOST: &str = "ctl_host";
const CONFIG_CTL_PORT: &str = "ctl_port";
const CONFIG_CTL_JWT: &str = "ctl_jwt";
const CONFIG_CTL_SEED: &str = "ctl_seed";
const CONFIG_CTL_CREDSFILE: &str = "ctl_credsfile";
const CONFIG_CTL_TLS_CA_FILE: &str = "ctl_tls_ca_file";
const CONFIG_CTL_TLS_FIRST: &str = "ctl_tls_first";
const CONFIG_JS_DOMAIN: &str = "js_domain";

fn default_lattice() -> String {
    DEFAULT_LATTICE.to_string()
}

fn default_ctl_host() -> String {
    DEFAULT_CTL_HOST.to_string()
}

fn default_ctl_port() -> u16 {
    DEFAULT_CTL_PORT
}

/// Configuration when subscribing a component with the
/// WADM provider as a source along a link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ClientConfig {
    /// The lattice this subscription is on.
    #[serde(default = "default_lattice")]
    pub lattice: String,
    /// Application name to subscribe to updates for.
    /// Cannot be empty if this is a subscription config.
    #[serde(default)]
    pub app_name: Option<String>,
    /// Control host for connection
    #[serde(default = "default_ctl_host")]
    pub ctl_host: String,
    /// Control port for connection
    #[serde(default = "default_ctl_port")]
    pub ctl_port: u16,
    /// JWT file for authentication
    #[serde(default)]
    pub ctl_jwt: Option<String>,
    /// Seed file/literal for authentication
    #[serde(default)]
    pub ctl_seed: Option<String>,
    /// Credentials file combining seed and JWT
    #[serde(default)]
    pub ctl_credsfile: Option<String>,
    /// TLS CA certificate file
    #[serde(default)]
    pub ctl_tls_ca_file: Option<String>,
    /// Perform TLS handshake first
    #[serde(default)]
    pub ctl_tls_first: bool,
    /// JetStream domain
    #[serde(default)]
    pub js_domain: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            lattice: default_lattice(),
            app_name: None,
            ctl_host: String::new(),
            ctl_port: 0,
            ctl_jwt: None,
            ctl_seed: None,
            ctl_credsfile: None,
            ctl_tls_ca_file: None,
            ctl_tls_first: false,
            js_domain: None,
        }
    }
}

impl TryFrom<HashMap<String, String>> for ClientConfig {
    type Error = anyhow::Error;

    fn try_from(values: HashMap<String, String>) -> Result<Self> {
        let mut config = ClientConfig::default();

        if let Some(ctl_host) = values.get(CONFIG_CTL_HOST) {
            config.ctl_host = ctl_host.clone();
        }
        if let Some(ctl_port) = values.get(CONFIG_CTL_PORT) {
            config.ctl_port = ctl_port.parse().map_err(|_| anyhow!("Invalid ctl_port"))?;
        }
        if let Some(ctl_jwt) = values.get(CONFIG_CTL_JWT) {
            config.ctl_jwt = Some(ctl_jwt.clone());
        }
        if let Some(ctl_seed) = values.get(CONFIG_CTL_SEED) {
            config.ctl_seed = Some(ctl_seed.clone());
        }
        if let Some(ctl_credsfile) = values.get(CONFIG_CTL_CREDSFILE) {
            config.ctl_credsfile = Some(ctl_credsfile.clone());
        }
        if let Some(ctl_tls_ca_file) = values.get(CONFIG_CTL_TLS_CA_FILE) {
            config.ctl_tls_ca_file = Some(ctl_tls_ca_file.clone());
        }
        if let Some(ctl_tls_first) = values.get(CONFIG_CTL_TLS_FIRST) {
            config.ctl_tls_first = ctl_tls_first
                .parse()
                .map_err(|_| anyhow!("Invalid ctl_tls_first value"))?;
        }
        if let Some(js_domain) = values.get(CONFIG_JS_DOMAIN) {
            config.js_domain = Some(js_domain.clone());
        }
        if let Some(lattice) = values.get(CONFIG_LATTICE) {
            config.lattice = lattice.clone();
        }
        if let Some(app_name) = values.get(CONFIG_APP_NAME) {
            config.app_name = Some(app_name.clone());
        }

        Ok(config)
    }
}

impl ClientConfig {
    pub fn merge(&self, extra: &ClientConfig) -> ClientConfig {
        let mut out = self.clone();

        if !extra.ctl_host.is_empty() {
            out.ctl_host = extra.ctl_host.clone();
        }
        if extra.ctl_port != 0 {
            out.ctl_port = extra.ctl_port;
        }
        if extra.ctl_jwt.is_some() {
            out.ctl_jwt = extra.ctl_jwt.clone();
        }
        if extra.ctl_seed.is_some() {
            out.ctl_seed = extra.ctl_seed.clone();
        }
        if extra.ctl_credsfile.is_some() {
            out.ctl_credsfile = extra.ctl_credsfile.clone();
        }
        if extra.ctl_tls_ca_file.is_some() {
            out.ctl_tls_ca_file = extra.ctl_tls_ca_file.clone();
        }
        if extra.ctl_tls_first {
            out.ctl_tls_first = extra.ctl_tls_first;
        }
        if extra.js_domain.is_some() {
            out.js_domain = extra.js_domain.clone();
        }
        if !extra.lattice.is_empty() {
            out.lattice = extra.lattice.clone();
        }
        if extra.app_name.is_some() {
            out.app_name = extra.app_name.clone();
        }

        out
    }
}

pub(crate) fn extract_wadm_config(
    link_config: &InterfaceConfig,
    is_subscription: bool,
) -> Option<ClientConfig> {
    // Convert config Vec to HashMap for easier access
    let config: HashMap<String, String> = link_config.config.iter().cloned().collect();
    let secrets = &link_config.secrets;
    let mut client_config = ClientConfig::default();

    // For subscriptions we need app_name
    if is_subscription {
        let app_name = config.get(CONFIG_APP_NAME);
        if app_name.is_none() {
            warn!("Subscription config missing required app_name field");
            return None;
        }
        client_config.app_name = app_name.cloned();
    }

    if let Some(host) = config.get(CONFIG_CTL_HOST) {
        client_config.ctl_host = host.clone();
    }
    if let Some(port) = config.get(CONFIG_CTL_PORT) {
        if let Ok(port_num) = port.parse() {
            client_config.ctl_port = port_num;
        }
    }

    if let Some(jwt_val) = config.get(CONFIG_CTL_JWT) {
        client_config.ctl_jwt = Some(jwt_val.clone());
    }

    // Handle seed (prefer secrets)
    if let Some(seed_secret) = secrets
        .as_ref()
        .and_then(|s| s.iter().find(|(k, _)| k == CONFIG_CTL_SEED))
        .and_then(|(_, v)| {
            let secret: SecretValue = v.into();
            secret.as_string().map(String::from)
        })
    {
        client_config.ctl_seed = Some(seed_secret);
    } else if let Some(seed_val) = config.get(CONFIG_CTL_SEED) {
        warn!("Seed found in config instead of secrets - consider moving to secrets");
        client_config.ctl_seed = Some(seed_val.clone());
    }

    if let Some(lattice) = config.get(CONFIG_LATTICE) {
        client_config.lattice = lattice.clone();
    }
    if let Some(credsfile) = config.get(CONFIG_CTL_CREDSFILE) {
        client_config.ctl_credsfile = Some(credsfile.clone());
    }
    if let Some(tls_ca_file) = config.get(CONFIG_CTL_TLS_CA_FILE) {
        client_config.ctl_tls_ca_file = Some(tls_ca_file.clone());
    }
    if let Some(tls_first) = config.get(CONFIG_CTL_TLS_FIRST) {
        client_config.ctl_tls_first = matches!(tls_first.to_lowercase().as_str(), "true" | "yes");
    }
    if let Some(js_domain) = config.get(CONFIG_JS_DOMAIN) {
        client_config.js_domain = Some(js_domain.clone());
    }

    Some(client_config)
}
