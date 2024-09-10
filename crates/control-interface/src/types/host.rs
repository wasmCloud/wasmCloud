//! Data types used for managing hosts on a wasmCloud lattice

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize, Serializer};

use crate::types::component::ComponentDescription;
use crate::types::provider::ProviderDescription;

/// A summary representation of a host
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Host {
    /// NATS server host used for regular RPC
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_host: Option<String>,
    /// NATS server host used for the control interface
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctl_host: Option<String>,
    /// Human-friendly name for this host
    #[serde(default)]
    pub friendly_name: String,
    /// Unique nkey public key for this host
    #[serde(default)]
    pub id: String,
    /// JetStream domain (if applicable) in use by this host
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub js_domain: Option<String>,
    /// Hash map of label-value pairs for this host
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// The lattice that this host is a member of
    #[serde(default)]
    pub lattice: String,
    /// Human-friendly uptime description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_human: Option<String>,
    /// uptime in seconds
    #[serde(default)]
    pub uptime_seconds: u64,
    /// Current wasmCloud Host software version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Describes the known contents of a given host at the time of
/// a query. Also used as a payload for the host heartbeat
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostInventory {
    /// Components running on this host.
    pub components: Vec<ComponentDescription>,
    /// Providers running on this host
    pub providers: Vec<ProviderDescription>,
    /// The host's unique ID
    #[serde(default)]
    pub host_id: String,
    /// The host's human-readable friendly name
    #[serde(default)]
    pub friendly_name: String,
    /// The host's labels
    #[serde(default)]
    #[serde(serialize_with = "serialize_as_btreemap")]
    pub labels: HashMap<String, String>,
    /// The host version
    #[serde(default)]
    pub version: String,
    /// The host uptime in human-readable form
    #[serde(default)]
    pub uptime_human: String,
    /// The host uptime in seconds
    #[serde(default)]
    pub uptime_seconds: u64,
}

/// Serde Serializer that works for sorting maps on the fly
fn serialize_as_btreemap<S, K, V>(value: &HashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    K: Serialize + Ord,
    V: Serialize,
{
    BTreeMap::from_iter(value.iter()).serialize(serializer)
}

/// A label on a given host (ex. "arch=amd64")
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostLabel {
    pub key: String,
    pub value: String,
}
