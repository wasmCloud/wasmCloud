//! Data types used in RPC calls (usually control-related) on a wasmCloud lattice

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{ComponentId, LinkName, WitNamespace, WitPackage};

/// A host response to a request to start an actor, confirming the host
/// has enough capacity to start the actor
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorAuctionAck {
    /// The original actor reference used for the auction
    #[serde(default)]
    pub actor_ref: String,
    /// The unique component identifier that the auctioner can use for this actor
    #[serde(default)]
    pub actor_id: String,
    /// The host ID of the "bidder" for this auction.
    #[serde(default)]
    pub host_id: String,
    /// Constraints that were used in the auction
    #[serde(default)]
    pub constraints: HashMap<String, String>,
}

/// A request to locate suitable hosts for a given actor
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorAuctionRequest {
    /// The image reference, file or OCI, for this actor.
    #[serde(default)]
    pub actor_ref: String,
    /// The unique identifier to be used for this actor. The host will ensure
    /// that no other actor with the same ID is running on the host
    pub actor_id: ComponentId,
    /// The set of constraints that must match the labels of a suitable target host
    pub constraints: HashMap<String, String>,
}

/// A host response to a request to start a provider, confirming the host
/// has enough capacity to start the provider and that the provider is
/// not already running on the host
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderAuctionAck {
    /// The host ID of the "bidder" for this auction
    #[serde(default)]
    pub host_id: String,
    /// The original provider reference provided for the auction
    #[serde(default)]
    pub provider_ref: String,
    /// The unique component identifier that the auctioner can use for this provider
    #[serde(default)]
    pub provider_id: String,
    /// The constraints provided for the auction
    #[serde(default)]
    pub constraints: HashMap<String, String>,
}

/// A request to locate a suitable host for a capability provider. The
/// provider's unique identity is used to rule out hosts on which the
/// provider is already running.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderAuctionRequest {
    /// The set of constraints that must match the labels of a suitable target host
    pub constraints: HashMap<String, String>,
    /// The image reference, file or OCI, for this provider.
    #[serde(default)]
    pub provider_ref: String,
    /// The unique identifier to be used for this provider. The host will ensure
    /// that no other provider with the same ID is running on the host
    pub provider_id: ComponentId,
}

/// A request to remove a link definition and detach the relevant actor
/// from the given provider
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeleteInterfaceLinkDefinitionRequest {
    /// The source component's identifier.
    pub source_id: ComponentId,
    /// Name of the link. Not providing this is equivalent to specifying Some("default")
    #[serde(default = "default_link_name")]
    pub name: LinkName,
    /// WIT namespace of the link, e.g. `wasi` in `wasi:keyvalue/readwrite.get`
    pub wit_namespace: WitNamespace,
    /// WIT package of the link, e.g. `keyvalue` in `wasi:keyvalue/readwrite.get`
    pub wit_package: WitPackage,
}

/// Helper function to provide a default link name
pub(crate) fn default_link_name() -> LinkName {
    "default".to_string()
}
