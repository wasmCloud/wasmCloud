//! Core reusable types related to performing [RPC calls on a wasmCloud lattice][docs-wasmcloud-rpc]
//!
//! Wasmbus is the name of the NATS-powered RPC transport mechanism primarily used by wasmCloud.
//!
//! Various wasmCloud workloads (capability providers, components) use Wasmbus (and thus NATS)
//! to communicate and send RPCs -- often over well known topics (some of which are detailed in this module).
//!
//! [docs-wasmcloud-rpc]: <https://wasmcloud.com/docs/hosts/lattice-protocols/rpc>

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HealthCheckRequest {}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HealthCheckResponse {
    /// A flag that indicates the component is healthy
    #[serde(default)]
    pub healthy: bool,
    /// A message containing additional information about the components health
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Generate the wasmbus RPC subject for putting links on a NATS cluster
///
/// When messages are published on this subject, hosts set up and update (if necessary) link information,
/// which may include calling `receive_link_config_*()` functions on relevant providers.
#[must_use]
pub fn link_put_subject(lattice: &str, provider_key: &str) -> String {
    format!("wasmbus.rpc.{lattice}.{provider_key}.linkdefs.put")
}

/// Generate the wasmbus RPC subject for deleting links on a NATS cluster
///
/// When messages are published on this subject, hosts remove link information,
/// which may include calling `delete_link()` on relevant providers.
#[must_use]
pub fn link_del_subject(lattice: &str, provider_key: &str) -> String {
    format!("wasmbus.rpc.{lattice}.{provider_key}.linkdefs.del")
}

/// Generate the wasmbus RPC subject for retrieving health information for a given provider
///
/// When messages are published on this subject, hosts trigger health checks on providers (i.e. a [`HealthCheckRequest`])
/// and return relevant results (i.e. a [`HealthCheckResponse`]).
#[must_use]
pub fn health_subject(lattice: &str, provider_key: &str) -> String {
    format!("wasmbus.rpc.{lattice}.{provider_key}.health")
}

/// Generate the wasmbus RPC subject for shutting down a given provider
///
/// When messages are published on this subject, hosts perform shutdown (cleanly if possible).
#[must_use]
pub fn shutdown_subject(lattice: &str, provider_key: &str, link_name: &str) -> String {
    format!("wasmbus.rpc.{lattice}.{provider_key}.{link_name}.shutdown")
}

/// Generate the wasmbus RPC subject for delivering config updates to a given provider
///
/// When messages are published on this subject, providers up the perform shutdown (cleanly if possible).
///
/// NOTE that the NATS message body limits (default 1MiB) apply to these messages
#[must_use]
pub fn provider_config_update_subject(lattice: &str, provider_key: &str) -> String {
    format!("wasmbus.rpc.{lattice}.{provider_key}.config.update")
}
