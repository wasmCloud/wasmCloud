//! Data types and structure used when managing links on a wasmCloud lattice

use serde::{Deserialize, Serialize};

use crate::{
    ComponentId, KnownConfigName, LatticeTarget, LinkName, WitInterface, WitNamespace, WitPackage,
};

/// A link definition between a source and target component (actor or provider) on a given
/// interface. An [`InterfaceLinkDefinition`] connects one component's import to another
/// component's export, specifying the configuration each component needs in order to execute
/// the request, and represents an operator's intent to allow the source to invoke the target.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize, Hash)]
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
    /// List of named configurations to provide to the source upon request
    #[serde(default)]
    pub source_config: Vec<KnownConfigName>,
    /// List of named configurations to provide to the target upon request
    #[serde(default)]
    pub target_config: Vec<KnownConfigName>,
}

/// Helper function to provide a default link name
pub(crate) fn default_link_name() -> LinkName {
    "default".to_string()
}
