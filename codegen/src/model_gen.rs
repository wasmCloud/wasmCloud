// This file contains structures copied from the compiled models
// wasmbus_model.rs, wasmbus_core.rs
// We can't link to that crate without creating a circular dependency,
// and keeping the copied structs in sync is relatively easier than
// doing a two-stage compile.
//
// All entities declared here should be pub(crate), to prevent them from
// being used by other crates. Other crates should use the compiled
// definitions in rpc-rs.
//
// Possible automated fixes:
//
// One (complicated) fix would be to have two versions of codegen library:
// a first-stage codegen with no dependencies that is used to generate the core lib, and
// a second-stage codegen that includes portions of the compiled core models,
// that compiles everything else.
// This would require building this crate with different sets of feature flags,
// and different dependencies.

// Another approach may be to create "plugins". where there is only one codegen build.
// first pass codegen, for the core libraries, uses no plugins.
// then plugins are compiled, incorporating compiled core libraries,
// then second pass codegen loads the plugins and compiles everything else
//

use serde::{Deserialize, Serialize};

/// Capability contract id, e.g. 'wasmcloud:httpserver'
pub(crate) type CapabilityContractId = String;

/// protocol definition trait for wasmbus services
/// This structure is copied from the generated wasmbus_core.rs
/// but we can't link to that trait without circular dependencies
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Wasmbus {
    /// capability id such as "wasmbus:httpserver"
    /// always required for providerReceive, but optional for actorReceive
    #[serde(rename = "contractId")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<CapabilityContractId>,
    /// indicates this service's operations are handled by an provider (default false)
    #[serde(rename = "providerReceive")]
    #[serde(default)]
    pub provider_receive: bool,
    /// indicates this service's operations are handled by an actor (default false)
    #[serde(rename = "actorReceive")]
    #[serde(default)]
    pub actor_receive: bool,
}

/// Overrides for serializer & deserializer
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Serialization {
    /// (optional setting) Override field name when serializing and deserializing
    /// By default, (when `name` not specified) is the exact declared name without
    /// casing transformations. This setting does not affect the field name
    /// produced in code generation, which is always lanaguage-idiomatic
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Rust codegen traits
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct CodegenRust {
    /// Instructs rust codegen to add `#[derive(Default)]` (default false)
    #[serde(rename = "deriveDefault")]
    #[serde(default)]
    pub derive_default: bool,
}
