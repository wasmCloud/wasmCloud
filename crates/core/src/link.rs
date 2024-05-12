//! Core reusable types related to links on wasmCloud lattices

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{ComponentId, LatticeTarget, WitInterface, WitNamespace, WitPackage};

/// Name of a link on the wasmCloud lattice
pub type LinkName = String;

/// A link definition between a source and target component (component or provider) on a given
/// interface. An [`InterfaceLinkDefinition`] connects one component's import to another
/// component's export, specifying the configuration each component needs in order to execute
/// the request, and represents an operator's intent to allow the source to invoke the target.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
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
}

/// Helper function to provide a default link name
pub(crate) fn default_link_name() -> LinkName {
    "default".to_string()
}
