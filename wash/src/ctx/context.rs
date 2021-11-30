use crate::{
    id::ClusterSeed,
    util::{DEFAULT_LATTICE_PREFIX, DEFAULT_NATS_HOST, DEFAULT_NATS_PORT, DEFAULT_NATS_TIMEOUT},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Deserialize, Serialize, Debug)]
pub(crate) struct DefaultContext {
    /// Name of the default context
    pub name: String,
}

impl DefaultContext {
    pub fn new(name: String) -> Self {
        DefaultContext { name }
    }
}

#[derive(Clone, Deserialize, Serialize, Debug)]
pub(crate) struct WashContext {
    #[serde(default)]
    pub name: String,
    #[serde(with = "serde_with::rust::string_empty_as_none")]
    pub cluster_seed: Option<ClusterSeed>,

    #[serde(default = "default_nats_host")]
    pub ctl_host: String,
    #[serde(default = "default_nats_port")]
    pub ctl_port: u16,
    #[serde(with = "serde_with::rust::string_empty_as_none")]
    pub ctl_jwt: Option<String>,
    #[serde(with = "serde_with::rust::string_empty_as_none")]
    pub ctl_seed: Option<String>,
    pub ctl_credsfile: Option<PathBuf>,
    #[serde(default = "default_timeout")]
    pub ctl_timeout: u64,

    #[serde(default = "default_lattice_prefix")]
    pub ctl_lattice_prefix: String,

    #[serde(default = "default_nats_host")]
    pub rpc_host: String,
    #[serde(default = "default_nats_port")]
    pub rpc_port: u16,
    #[serde(with = "serde_with::rust::string_empty_as_none")]
    pub rpc_jwt: Option<String>,
    #[serde(with = "serde_with::rust::string_empty_as_none")]
    pub rpc_seed: Option<String>,
    pub rpc_credsfile: Option<PathBuf>,
    #[serde(default = "default_timeout")]
    pub rpc_timeout: u64,

    #[serde(default = "default_lattice_prefix")]
    pub rpc_lattice_prefix: String,
}

impl WashContext {
    pub(crate) fn named(name: String) -> Self {
        WashContext {
            name,
            ..Self::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        name: String,
        cluster_seed: Option<ClusterSeed>,
        ctl_host: String,
        ctl_port: u16,
        ctl_jwt: Option<String>,
        ctl_seed: Option<String>,
        ctl_credsfile: Option<PathBuf>,
        ctl_timeout: u64,
        ctl_lattice_prefix: String,
        rpc_host: String,
        rpc_port: u16,
        rpc_jwt: Option<String>,
        rpc_seed: Option<String>,
        rpc_credsfile: Option<PathBuf>,
        rpc_timeout: u64,
        rpc_lattice_prefix: String,
    ) -> Self {
        WashContext {
            name,
            cluster_seed,
            ctl_host,
            ctl_port,
            ctl_jwt,
            ctl_seed,
            ctl_credsfile,
            ctl_timeout,
            ctl_lattice_prefix,
            rpc_host,
            rpc_port,
            rpc_jwt,
            rpc_seed,
            rpc_credsfile,
            rpc_timeout,
            rpc_lattice_prefix,
        }
    }
}

impl Default for WashContext {
    fn default() -> Self {
        WashContext {
            name: "default".to_string(),
            cluster_seed: None,
            ctl_host: DEFAULT_NATS_HOST.to_string(),
            ctl_port: DEFAULT_NATS_PORT.parse().unwrap(),
            ctl_jwt: None,
            ctl_seed: None,
            ctl_credsfile: None,
            ctl_timeout: DEFAULT_NATS_TIMEOUT,
            ctl_lattice_prefix: DEFAULT_LATTICE_PREFIX.to_string(),
            rpc_host: DEFAULT_NATS_HOST.to_string(),
            rpc_port: DEFAULT_NATS_PORT.parse().unwrap(),
            rpc_jwt: None,
            rpc_seed: None,
            rpc_credsfile: None,
            rpc_timeout: DEFAULT_NATS_TIMEOUT,
            rpc_lattice_prefix: DEFAULT_LATTICE_PREFIX.to_string(),
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

fn default_lattice_prefix() -> String {
    DEFAULT_LATTICE_PREFIX.to_string()
}

fn default_timeout() -> u64 {
    DEFAULT_NATS_TIMEOUT
}
