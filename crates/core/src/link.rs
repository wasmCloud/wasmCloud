//! Core reusable types related to [links on wasmCloud lattices][docs-links]
//!
//! [docs-links]: <https://wasmcloud.com/docs/concepts/linking-components>

use std::collections::HashMap;

use secrecy::zeroize::{Zeroize, ZeroizeOnDrop};
use serde::{Deserialize, Serialize};

use crate::{
    secrets::SecretValue, ComponentId, LatticeTarget, WitInterface, WitNamespace, WitPackage,
};

/// Name of a link on the wasmCloud lattice
pub type LinkName = String;

/// A link definition between a source and target component (component or provider) on a given
/// interface. An [`InterfaceLinkDefinition`] connects one component's import to another
/// component's export, specifying the configuration each component needs in order to execute
/// the request, and represents an operator's intent to allow the source to invoke the target.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct InterfaceLinkDefinition {
    /// Source identifier for the link
    pub source_id: ComponentId,
    /// Target for the link, which can be a unique identifier or (future) a routing group
    pub target: LatticeTarget,
    /// Name of the link. Not providing this is equivalent to specifying "default"
    #[serde(default = "default_link_name")]
    pub name: LinkName,
    /// WIT namespace of the link operation, e.g. `wasi` in `wasi:keyvalue/readwrite.get`
    pub wit_namespace: WitNamespace,
    /// WIT package of the link operation, e.g. `keyvalue` in `wasi:keyvalue/readwrite.get`
    pub wit_package: WitPackage,
    /// WIT Interfaces to be used for the link, e.g. `readwrite`, `atomic`, etc.
    pub interfaces: Vec<WitInterface>,
    /// The configuration to give to the source for this link
    #[serde(default)]
    pub source_config: HashMap<String, String>,
    /// The configuration to give to the target for this link
    #[serde(default)]
    pub target_config: HashMap<String, String>,
    /// The secrets to give to the source of this link
    #[serde(default)]
    pub source_secrets: Option<HashMap<String, SecretValue>>,
    /// The secrets to give to the target of this link
    #[serde(default)]
    pub target_secrets: Option<HashMap<String, SecretValue>>,
}

// Trait implementations that ensure we zeroize secrets when they are dropped
impl ZeroizeOnDrop for InterfaceLinkDefinition {}
impl Zeroize for InterfaceLinkDefinition {
    fn zeroize(&mut self) {
        if let Some(ref mut secrets) = self.source_secrets {
            for (_, secret_value) in secrets.iter_mut() {
                secret_value.zeroize();
            }
            secrets.clear();
        }
        if let Some(ref mut secrets) = self.target_secrets {
            for (_, secret_value) in secrets.iter_mut() {
                secret_value.zeroize();
            }
            secrets.clear();
        }
    }
}

impl Zeroize for SecretValue {
    fn zeroize(&mut self) {
        match self {
            SecretValue::String(ref mut s) => {
                s.zeroize();
            }
            SecretValue::Bytes(ref mut bytes) => {
                bytes.zeroize();
            }
        }
    }
}

/// Helper function to provide a default link name
pub(crate) fn default_link_name() -> LinkName {
    "default".to_string()
}
