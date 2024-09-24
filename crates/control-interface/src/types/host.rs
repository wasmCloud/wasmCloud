//! Data types used for managing hosts on a wasmCloud lattice

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::component::ComponentDescription;
use crate::types::provider::ProviderDescription;
use crate::Result;

/// A summary representation of a host
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Host {
    /// NATS server host used for regular RPC
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rpc_host: Option<String>,

    /// NATS server host used for the control interface
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ctl_host: Option<String>,

    /// Human-friendly name for this host
    #[serde(default)]
    pub(crate) friendly_name: String,

    /// Unique nkey public key for this host
    #[serde(default)]
    pub(crate) id: String,

    /// JetStream domain (if applicable) in use by this host
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) js_domain: Option<String>,

    /// Hash map of label-value pairs for this host
    #[serde(default)]
    pub(crate) labels: BTreeMap<String, String>,

    /// The lattice that this host is a member of
    #[serde(default)]
    pub(crate) lattice: String,

    /// Human-friendly uptime description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) uptime_human: Option<String>,

    /// Uptime in seconds
    #[serde(default)]
    pub(crate) uptime_seconds: u64,

    /// Current wasmCloud Host software version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
}

impl Host {
    /// Get the NATS server host used for RPC
    pub fn rpc_host(&self) -> Option<&str> {
        self.rpc_host.as_deref()
    }

    /// Get the NATS server host used for control interface commands
    pub fn ctl_host(&self) -> Option<&str> {
        self.ctl_host.as_deref()
    }

    /// Get the friendly name of the host
    pub fn friendly_name(&self) -> &str {
        &self.friendly_name
    }

    /// Get the ID of the host
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the NATS Jetstream domain for the host
    pub fn js_domain(&self) -> Option<&str> {
        self.js_domain.as_deref()
    }

    /// Get the labels on the host
    pub fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }

    /// Get the lattice this host is a member of
    pub fn lattice(&self) -> &str {
        &self.lattice
    }

    /// Get a human friendly host uptime description
    pub fn uptime_human(&self) -> Option<&str> {
        self.uptime_human.as_deref()
    }

    /// Get the number of seconds the host has been up
    pub fn uptime_seconds(&self) -> u64 {
        self.uptime_seconds
    }

    /// Get the version of the host
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    #[must_use]
    pub fn builder() -> HostBuilder {
        HostBuilder::default()
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostBuilder {
    rpc_host: Option<String>,
    ctl_host: Option<String>,
    friendly_name: Option<String>,
    id: Option<String>,
    js_domain: Option<String>,
    labels: Option<BTreeMap<String, String>>,
    lattice: Option<String>,
    uptime_human: Option<String>,
    uptime_seconds: Option<u64>,
    version: Option<String>,
}

impl HostBuilder {
    #[must_use]
    pub fn rpc_host(mut self, v: String) -> Self {
        self.rpc_host = Some(v);
        self
    }

    #[must_use]
    pub fn ctl_host(mut self, v: String) -> Self {
        self.ctl_host = Some(v);
        self
    }

    #[must_use]
    pub fn friendly_name(mut self, v: String) -> Self {
        self.friendly_name = Some(v);
        self
    }

    #[must_use]
    pub fn id(mut self, v: String) -> Self {
        self.id = Some(v);
        self
    }

    #[must_use]
    pub fn js_domain(mut self, v: String) -> Self {
        self.js_domain = Some(v);
        self
    }

    #[must_use]
    pub fn lattice(mut self, v: String) -> Self {
        self.lattice = Some(v);
        self
    }

    #[must_use]
    pub fn labels(mut self, v: BTreeMap<String, String>) -> Self {
        self.labels = Some(v);
        self
    }

    #[must_use]
    pub fn uptime_human(mut self, v: String) -> Self {
        self.uptime_human = Some(v);
        self
    }

    #[must_use]
    pub fn uptime_seconds(mut self, v: u64) -> Self {
        self.uptime_seconds = Some(v);
        self
    }

    #[must_use]
    pub fn version(mut self, v: String) -> Self {
        self.version = Some(v);
        self
    }

    pub fn build(self) -> Result<Host> {
        Ok(Host {
            friendly_name: self
                .friendly_name
                .ok_or_else(|| "friendly_name is required".to_string())?,
            labels: self.labels.unwrap_or_default(),
            uptime_human: self.uptime_human,
            uptime_seconds: self
                .uptime_seconds
                .ok_or_else(|| "uptime_seconds is required".to_string())?,
            rpc_host: self.rpc_host,
            ctl_host: self.ctl_host,
            id: self.id.ok_or_else(|| "id is required".to_string())?,
            lattice: self
                .lattice
                .ok_or_else(|| "lattice is required".to_string())?,
            js_domain: self.js_domain,
            version: self.version,
        })
    }
}

/// Describes the known contents of a given host at the time of
/// a query. Also used as a payload for the host heartbeat
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct HostInventory {
    /// Components running on this host.
    #[serde(alias = "actors")]
    pub(crate) components: Vec<ComponentDescription>,

    /// Providers running on this host
    pub(crate) providers: Vec<ProviderDescription>,

    /// The host's unique ID
    #[serde(default)]
    pub(crate) host_id: String,

    /// The host's human-readable friendly name
    #[serde(default)]
    pub(crate) friendly_name: String,

    /// The host's labels
    #[serde(default)]
    pub(crate) labels: BTreeMap<String, String>,

    /// The host version
    #[serde(default)]
    pub(crate) version: String,

    /// The host uptime in human-readable form
    #[serde(default)]
    pub(crate) uptime_human: String,

    /// The host uptime in seconds
    #[serde(default)]
    pub(crate) uptime_seconds: u64,
}

impl HostInventory {
    /// Get information about providers in the inventory
    pub fn components(&self) -> &Vec<ComponentDescription> {
        self.components.as_ref()
    }

    /// Get information about providers in the inventory
    pub fn providers(&self) -> &Vec<ProviderDescription> {
        &self.providers
    }

    /// Get the ID of the host from which this inventory was returned
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    /// Get the friendly name of the host
    pub fn friendly_name(&self) -> &str {
        &self.friendly_name
    }

    /// Get the labels on the host
    pub fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }

    /// Get the version of the host
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Get a human friendly host uptime description
    pub fn uptime_human(&self) -> &str {
        &self.uptime_human
    }

    /// Get the number of seconds the host has been up
    pub fn uptime_seconds(&self) -> u64 {
        self.uptime_seconds
    }

    #[must_use]
    pub fn builder() -> HostInventoryBuilder {
        HostInventoryBuilder::default()
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostInventoryBuilder {
    components: Option<Vec<ComponentDescription>>,
    providers: Option<Vec<ProviderDescription>>,
    host_id: Option<String>,
    friendly_name: Option<String>,
    labels: Option<BTreeMap<String, String>>,
    version: Option<String>,
    uptime_human: Option<String>,
    uptime_seconds: Option<u64>,
}

impl HostInventoryBuilder {
    #[must_use]
    pub fn friendly_name(mut self, v: String) -> Self {
        self.friendly_name = Some(v);
        self
    }

    #[must_use]
    pub fn host_id(mut self, v: String) -> Self {
        self.host_id = Some(v);
        self
    }

    #[must_use]
    pub fn version(mut self, v: String) -> Self {
        self.version = Some(v);
        self
    }

    #[must_use]
    pub fn components(mut self, v: Vec<ComponentDescription>) -> Self {
        self.components = Some(v);
        self
    }

    #[must_use]
    pub fn providers(mut self, v: Vec<ProviderDescription>) -> Self {
        self.providers = Some(v);
        self
    }

    #[must_use]
    pub fn uptime_human(mut self, v: String) -> Self {
        self.uptime_human = Some(v);
        self
    }

    #[must_use]
    pub fn uptime_seconds(mut self, v: u64) -> Self {
        self.uptime_seconds = Some(v);
        self
    }

    #[must_use]
    pub fn labels(mut self, v: BTreeMap<String, String>) -> Self {
        self.labels = Some(v);
        self
    }

    pub fn build(self) -> Result<HostInventory> {
        Ok(HostInventory {
            components: self.components.unwrap_or_default(),
            providers: self.providers.unwrap_or_default(),
            host_id: self
                .host_id
                .ok_or_else(|| "host_id is required".to_string())?,
            friendly_name: self
                .friendly_name
                .ok_or_else(|| "friendly_name is required".to_string())?,
            labels: self.labels.unwrap_or_default(),
            version: self
                .version
                .ok_or_else(|| "version is required".to_string())?,
            uptime_human: self
                .uptime_human
                .ok_or_else(|| "uptime_human is required".to_string())?,
            uptime_seconds: self
                .uptime_seconds
                .ok_or_else(|| "uptime_seconds is required".to_string())?,
        })
    }
}

/// A label on a given host (ex. "arch=amd64")
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct HostLabel {
    /// Key of the label (`arch` in `arch=amd64`)
    pub(crate) key: String,

    /// Value of the label (`amd64` in `arch=amd64`)
    pub(crate) value: String,
}

impl HostLabel {
    /// Create a [`HostLabel`] from a key and value
    pub fn from_kv(key: &str, value: &str) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }

    /// Get the host label key
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Get the host label value
    pub fn value(&self) -> &str {
        &self.value
    }
}
