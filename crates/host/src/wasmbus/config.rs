use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::oci::Config as OciConfig;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// wasmCloud Host configuration
pub struct Host {
    /// NATS URL to connect to for control interface connection
    pub ctl_nats_url: Url,
    /// Authentication JWT for control interface connection, must be specified with ctl_seed
    pub ctl_jwt: Option<String>,
    /// Authentication NKEY Seed for control interface connection, must be specified with ctl_jwt
    pub ctl_seed: Option<String>,
    /// Whether to require TLS for control interface connection
    pub ctl_tls: bool,
    /// The topic prefix to use for control interface subscriptions, defaults to `wasmbus.ctl`
    pub ctl_topic_prefix: String,
    /// NATS URL to connect to for actor RPC
    pub rpc_nats_url: Url,
    /// Timeout period for all RPC calls
    pub rpc_timeout: Duration,
    /// Authentication JWT for RPC connection, must be specified with rpc_seed
    pub rpc_jwt: Option<String>,
    /// Authentication NKEY Seed for RPC connection, must be specified with rpc_jwt
    pub rpc_seed: Option<String>,
    /// Whether to require TLS for RPC connection
    pub rpc_tls: bool,
    /// NATS URL to pass to providers for RPC
    pub prov_rpc_nats_url: Url,
    /// Authentication JWT for Provider RPC connection, must be specified with prov_rpc_seed
    pub prov_rpc_jwt: Option<String>,
    /// Authentication NKEY Seed for Provider RPC connection, must be specified with prov_rpc_jwt
    pub prov_rpc_seed: Option<String>,
    /// Whether to require TLS for Provider RPC connection
    pub prov_rpc_tls: bool,
    /// The lattice the host belongs to
    pub lattice_prefix: String,
    /// The domain to use for host Jetstream operations
    pub js_domain: Option<String>,
    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to generate its public key
    pub host_seed: Option<String>,
    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to sign all invocations
    pub cluster_seed: Option<String>,
    /// The identity keys (a printable 256-bit Ed25519 public key) that this host should allow invocations from
    pub cluster_issuers: Option<Vec<String>>,
    /// The amount of time to wait for a provider to gracefully shut down before terminating it
    pub provider_shutdown_delay: Option<std::time::Duration>,
    /// Configuration for downloading artifacts from OCI registries
    pub oci_opts: OciConfig,
    /// Whether to allow loading actor or provider components from the filesystem
    pub allow_file_load: bool,
    // Whether or not structured logging is enabled
    // pub enable_structured_logging: bool,
    // Log level to pass to capability providers to use. Should be parsed from a [`tracing::Level`]
    // pub log_level: String,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            ctl_nats_url: Url::parse("nats://localhost:4222")
                .expect("failed to parse control NATS URL"),
            ctl_jwt: None,
            ctl_seed: None,
            ctl_tls: false,
            ctl_topic_prefix: "wasmbus.ctl".to_string(),
            rpc_nats_url: Url::parse("nats://localhost:4222")
                .expect("failed to parse RPC NATS URL"),
            rpc_timeout: Duration::from_millis(2000),
            rpc_jwt: None,
            rpc_seed: None,
            rpc_tls: false,
            prov_rpc_nats_url: Url::parse("nats://localhost:4222")
                .expect("failed to parse Provider RPC NATS URL"),
            prov_rpc_jwt: None,
            prov_rpc_seed: None,
            prov_rpc_tls: false,
            lattice_prefix: "default".to_string(),
            js_domain: None,
            host_seed: None,
            cluster_seed: None,
            cluster_issuers: None,
            provider_shutdown_delay: None,
            oci_opts: OciConfig::default(),
            allow_file_load: false,
            // enable_structured_logging: false,
            // log_level: "INFO".to_string(),
        }
    }
}
