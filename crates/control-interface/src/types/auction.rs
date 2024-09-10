//! Data types used in RPC calls (usually control-related) on a wasmCloud lattice

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ComponentId;

/// A host response to a request to start an component, confirming the host
/// has enough capacity to start the component
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ComponentAuctionAck {
    /// The original component reference used for the auction
    #[serde(default)]
    pub component_ref: String,
    /// The unique component identifier that the auctioner can use for this component
    #[serde(default)]
    pub component_id: String,
    /// The host ID of the "bidder" for this auction.
    #[serde(default)]
    pub host_id: String,
    /// Constraints that were used in the auction
    #[serde(default)]
    pub constraints: HashMap<String, String>,
    /// The maximum linear memory that an instance of this component can consume
    #[serde(default)]
    pub max_memory: u64,
    /// The maximum number of instances of this component that can be run on a host
    #[serde(default)]
    pub max_instances: u32,
}

/// A request to locate suitable hosts for a given component
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ComponentAuctionRequest {
    /// The image reference, file or OCI, for this component.
    pub component_ref: String,
    /// The unique identifier to be used for this component. The host will ensure
    /// that no other component with the same ID is running on the host
    pub component_id: ComponentId,
    /// The set of constraints that must match the labels of a suitable target host
    #[serde(default)]
    pub constraints: HashMap<String, String>,
    /// The maximum linear memory that an instance of this component can consume
    #[serde(default)]
    pub max_memory: u64,
    /// The maximum number of instances of this component that can be run on a host, defaults to `1`
    #[serde(default = "default_instances")]
    pub max_instances: u32,
}

/// By default an auction is asking at least for a single instance. This function
/// is included to ensure that auctioning an instance without specifying the number
/// of instances is combined with the max_memory field.
fn default_instances() -> u32 {
    1
}

/// A builder for `ComponentAuctionRequest`
#[derive(Debug)]
pub struct ComponentAuctionRequestBuilder {
    request: ComponentAuctionRequest,
}

impl ComponentAuctionRequestBuilder {
    /// Creates a new `ComponentAuctionRequestBuilder` instance
    pub fn new(component_ref: impl AsRef<str>, component_id: impl AsRef<str>) -> Self {
        let request = ComponentAuctionRequest {
            component_ref: component_ref.as_ref().to_string(),
            component_id: component_id.as_ref().to_string(),
            constraints: HashMap::new(),
            max_memory: 0,
            max_instances: 1,
        };
        ComponentAuctionRequestBuilder { request }
    }

    /// Sets the constraints for the `ComponentAuctionRequest`
    pub fn constraints(mut self, constraints: HashMap<String, String>) -> Self {
        self.request.constraints = constraints;
        self
    }

    /// Sets the maximum memory for the `ComponentAuctionRequest`
    pub fn max_memory(mut self, max_memory: u64) -> Self {
        self.request.max_memory = max_memory;
        self
    }

    /// Sets the maximum instances for the `ComponentAuctionRequest`
    pub fn max_instances(mut self, max_instances: u32) -> Self {
        self.request.max_instances = max_instances;
        self
    }

    /// Builds the `ComponentAuctionRequest`
    pub fn build(self) -> ComponentAuctionRequest {
        self.request
    }
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
