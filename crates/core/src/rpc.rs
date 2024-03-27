use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HealthCheckRequest {}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HealthCheckResponse {
    /// A flag that indicates the the actor is healthy
    #[serde(default)]
    pub healthy: bool,
    /// A message containing additional information about the actors health
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Generate the wasmbus RPC subject for putting links on a NATS cluster
///
/// When messages are published on this subject, hosts set up and update (if necessary) link information,
/// which may include calling `receive_link_config_*()` functions on relevant providers.
pub fn link_put_subject(lattice: &str, provider_key: &str) -> String {
    format!("wasmbus.rpc.{lattice}.{provider_key}.linkdefs.put")
}

/// Generate the wasmbus RPC subject for deleting links on a NATS cluster
///
/// When messages are published on this subject, hosts remove link information,
/// which may include calling `delete_link()` on relevant providers.
pub fn link_del_subject(lattice: &str, provider_key: &str) -> String {
    format!("wasmbus.rpc.{lattice}.{provider_key}.linkdefs.del")
}

/// Generate the wasmbus RPC subject for retrieving health information for a given provider
///
/// When messages are published on this subject, hosts trigger health checks on providers (i.e. a [`HealthCheckRequest`])
/// and return relevant results (i.e. a [`HealthCheckResponse`]).
pub fn health_subject(lattice: &str, provider_key: &str) -> String {
    format!("wasmbus.rpc.{lattice}.{provider_key}.health")
}

/// Generate the wasmbus RPC subject for shutting down a given provider
///
/// When messages are published on this subject, hosts perform shutdown (cleanly if possible).
pub fn shutdown_subject(lattice: &str, provider_key: &str, link_name: &str) -> String {
    format!("wasmbus.rpc.{lattice}.{provider_key}.{link_name}.shutdown")
}
