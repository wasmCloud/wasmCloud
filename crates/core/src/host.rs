//! Core reusable functionality related to hosts

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::lattice::ClusterIssuerKey;
use crate::link::InterfaceLinkDefinition;
use crate::logging::Level;
use crate::otel::OtelConfig;
use crate::wit::{deserialize_wit_map, serialize_wit_map, WitMap};

/// Environment settings for initializing a capability provider
pub type HostEnvValues = WitMap<String>;

/// initialization data for a capability provider
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HostData {
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub lattice_rpc_prefix: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub lattice_rpc_user_jwt: String,
    #[serde(default)]
    pub lattice_rpc_user_seed: String,
    #[serde(default)]
    pub lattice_rpc_url: String,
    #[serde(default)]
    pub provider_key: String,
    #[serde(default)]
    pub invocation_seed: String,
    #[serde(
        serialize_with = "serialize_wit_map",
        deserialize_with = "deserialize_wit_map"
    )]
    pub env_values: HostEnvValues,
    #[serde(default)]
    pub instance_id: String,
    /// initial list of links for provider
    pub link_definitions: Vec<InterfaceLinkDefinition>,
    /// list of cluster issuers
    pub cluster_issuers: Vec<ClusterIssuerKey>,
    /// Merged named configuration set for this provider at runtime
    #[serde(default)]
    pub config: HashMap<String, String>,
    /// Host-wide default RPC timeout for rpc messages, in milliseconds.  Defaults to 2000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_rpc_timeout_ms: Option<u64>,
    /// True if structured logging is enabled for the host. Providers should use the same setting as the host.
    #[serde(default)]
    pub structured_logging: bool,
    /// The log level providers should log at
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<Level>,
    pub otel_config: OtelConfig,
}
