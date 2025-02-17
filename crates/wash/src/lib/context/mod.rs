//! Types and methods for handling wash contexts, the configuration files for interacting with
//! lattices

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, NoneAsEmptyString};

use crate::lib::{
    config::{
        DEFAULT_COMPONENT_OPERATION_TIMEOUT_MS, DEFAULT_LATTICE, DEFAULT_NATS_HOST,
        DEFAULT_NATS_PORT, DEFAULT_NATS_TIMEOUT_MS,
    },
    id::ClusterSeed,
};

pub mod fs;

pub const HOST_CONFIG_NAME: &str = "host_config";

/// A trait that can be implemented by any type that wants to load, save, and otherwise manage wash
/// contexts (e.g. from a database or a config store
// NOTE(thomastaylor312): We may want to make this an async trait in the future since any other
// implementation than the fs one will likely involve networking
pub trait ContextManager {
    /// Returns the name of the currently set default context.
    fn default_context_name(&self) -> Result<String>;

    /// Sets the current default context to the given name. Should error if it doesn't exist
    fn set_default_context(&self, name: &str) -> Result<()>;

    /// Saves the given context
    fn save_context(&self, ctx: &WashContext) -> Result<()>;

    /// Deletes named context. If this context is the current default context, the default context
    /// should be unset
    fn delete_context(&self, name: &str) -> Result<()>;

    /// Loads the currently set default context
    fn load_default_context(&self) -> Result<WashContext>;

    /// Loads the named context
    fn load_context(&self, name: &str) -> Result<WashContext>;

    /// Returns a list of all context names
    fn list_contexts(&self) -> Result<Vec<String>>;
}

#[serde_as]
#[derive(Clone, Deserialize, Serialize, Debug)]
pub struct WashContext {
    #[serde(default)]
    pub name: String,
    #[serde_as(as = "NoneAsEmptyString")]
    pub cluster_seed: Option<ClusterSeed>,

    #[serde(default = "default_nats_host")]
    pub ctl_host: String,
    #[serde(default = "default_nats_port")]
    pub ctl_port: u16,
    #[serde_as(as = "NoneAsEmptyString")]
    pub ctl_jwt: Option<String>,
    #[serde_as(as = "NoneAsEmptyString")]
    pub ctl_seed: Option<String>,
    pub ctl_credsfile: Option<PathBuf>,
    /// timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub ctl_timeout: u64,
    /// TLS CA file to use for CTL
    pub ctl_tls_ca_file: Option<PathBuf>,
    /// Perform TLS handshake before expecting the server greeting for CTL
    pub ctl_tls_first: Option<bool>,

    // NOTE: lattice_prefix was renamed to lattice in most places, but this alias will need to remain for backwards compatibility with existing context files
    #[serde(alias = "lattice_prefix", default = "default_lattice")]
    pub lattice: String,

    pub js_domain: Option<String>,

    #[serde(default = "default_nats_host")]
    pub rpc_host: String,
    #[serde(default = "default_nats_port")]
    pub rpc_port: u16,
    #[serde_as(as = "NoneAsEmptyString")]
    pub rpc_jwt: Option<String>,
    #[serde_as(as = "NoneAsEmptyString")]
    pub rpc_seed: Option<String>,
    pub rpc_credsfile: Option<PathBuf>,
    /// rpc timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub rpc_timeout: u64,
    /// TLS CA file to use for RPC calls
    pub rpc_tls_ca_file: Option<PathBuf>,
    /// Perform TLS handshake before expecting the server greeting for RPC
    pub rpc_tls_first: Option<bool>,
}

impl WashContext {
    /// Create a new default context with the given name
    #[must_use]
    pub fn named(name: String) -> Self {
        Self {
            name,
            ..Self::default()
        }
    }
}

impl Default for WashContext {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            cluster_seed: None,
            ctl_host: DEFAULT_NATS_HOST.to_string(),
            ctl_port: DEFAULT_NATS_PORT.parse().unwrap(),
            ctl_jwt: None,
            ctl_seed: None,
            ctl_credsfile: None,
            ctl_timeout: DEFAULT_NATS_TIMEOUT_MS,
            ctl_tls_ca_file: None,
            ctl_tls_first: None,
            lattice: DEFAULT_LATTICE.to_string(),
            js_domain: None,
            rpc_host: DEFAULT_NATS_HOST.to_string(),
            rpc_port: DEFAULT_NATS_PORT.parse().unwrap(),
            rpc_jwt: None,
            rpc_seed: None,
            rpc_credsfile: None,
            rpc_timeout: DEFAULT_NATS_TIMEOUT_MS,
            rpc_tls_ca_file: None,
            rpc_tls_first: None,
        }
    }
}

// Below are required functions for serde default derive with WashContext

fn default_nats_host() -> String {
    DEFAULT_NATS_HOST.to_string()
}

fn default_nats_port() -> u16 {
    DEFAULT_NATS_PORT.parse().unwrap()
}

fn default_lattice() -> String {
    DEFAULT_LATTICE.to_string()
}

#[must_use]
pub const fn default_timeout_ms() -> u64 {
    DEFAULT_NATS_TIMEOUT_MS
}

/// Default timeout that should be used with operations that manipulate components (ex. scale)
#[must_use]
pub const fn default_component_operation_timeout_ms() -> u64 {
    DEFAULT_COMPONENT_OPERATION_TIMEOUT_MS
}
