//! Data types used in RPC calls (usually control-related) on a wasmCloud lattice

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Result;

/// A host response to a request to start a component.
///
/// This acknowledgement confirms that the host has enough capacity to start the component
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ComponentAuctionAck {
    /// The original component reference used for the auction
    #[serde(default)]
    pub(crate) component_ref: String,
    /// The unique component identifier that the auctioner can use for this component
    #[serde(default)]
    pub(crate) component_id: String,
    /// The host ID of the "bidder" for this auction.
    #[serde(default)]
    pub(crate) host_id: String,
    /// Constraints that were used in the auction
    #[serde(default)]
    pub(crate) constraints: BTreeMap<String, String>,
}

impl ComponentAuctionAck {
    #[must_use]
    pub fn from_component_host_and_constraints(
        component_ref: &str,
        component_id: &str,
        host_id: &str,
        constraints: impl Into<BTreeMap<String, String>>,
    ) -> Self {
        Self {
            component_ref: component_ref.into(),
            component_id: component_id.into(),
            host_id: host_id.into(),
            constraints: constraints.into(),
        }
    }

    /// Get the component ref for the auction acknowledgement
    #[must_use]
    pub fn component_ref(&self) -> &str {
        self.component_ref.as_ref()
    }

    /// Get the component ID for the auction acknowledgement
    #[must_use]
    pub fn component_id(&self) -> &str {
        self.component_ref.as_ref()
    }

    /// Get the host ID for the auction acknowledgement
    #[must_use]
    pub fn host_id(&self) -> &str {
        self.host_id.as_ref()
    }

    /// Get the constraints acknowledged by the auction acknowledgement
    #[must_use]
    pub fn constraints(&self) -> &BTreeMap<String, String> {
        &self.constraints
    }

    pub fn builder() -> ComponentAuctionAckBuilder {
        ComponentAuctionAckBuilder::default()
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
pub struct ComponentAuctionAckBuilder {
    component_ref: Option<String>,
    component_id: Option<String>,
    host_id: Option<String>,
    constraints: Option<BTreeMap<String, String>>,
}

impl ComponentAuctionAckBuilder {
    #[must_use]
    pub fn component_ref(mut self, v: String) -> Self {
        self.component_ref = Some(v);
        self
    }

    #[must_use]
    pub fn component_id(mut self, v: String) -> Self {
        self.component_id = Some(v);
        self
    }

    #[must_use]
    pub fn host_id(mut self, v: String) -> Self {
        self.host_id = Some(v);
        self
    }

    #[must_use]
    pub fn constraints(mut self, v: BTreeMap<String, String>) -> Self {
        self.constraints = Some(v);
        self
    }

    pub fn build(self) -> Result<ComponentAuctionAck> {
        Ok(ComponentAuctionAck {
            component_ref: self
                .component_ref
                .ok_or_else(|| "component_ref is required".to_string())?,
            component_id: self
                .component_id
                .ok_or_else(|| "component_id is required".to_string())?,
            host_id: self
                .host_id
                .ok_or_else(|| "host_id is required".to_string())?,
            constraints: self.constraints.unwrap_or_default(),
        })
    }
}

/// A request to locate suitable hosts for a given component
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ComponentAuctionRequest {
    /// The image reference, file or OCI, for this component.
    #[serde(default)]
    pub(crate) component_ref: String,
    /// The unique identifier to be used for this component.
    ///
    /// The host will ensure that no other component with the same ID is running on the host
    pub(crate) component_id: String,
    /// The set of constraints that must match the labels of a suitable target host
    pub(crate) constraints: BTreeMap<String, String>,
}

impl ComponentAuctionRequest {
    /// Get the component ref for the auction request
    #[must_use]
    pub fn component_ref(&self) -> &str {
        self.component_ref.as_ref()
    }

    /// Get the component ID for the auction request
    #[must_use]
    pub fn component_id(&self) -> &str {
        self.component_ref.as_ref()
    }

    /// Get the constraints for the auction request
    #[must_use]
    pub fn constraints(&self) -> &BTreeMap<String, String> {
        &self.constraints
    }

    pub fn builder() -> ComponentAuctionRequestBuilder {
        ComponentAuctionRequestBuilder::default()
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
pub struct ComponentAuctionRequestBuilder {
    component_ref: Option<String>,
    component_id: Option<String>,
    constraints: Option<BTreeMap<String, String>>,
}

impl ComponentAuctionRequestBuilder {
    #[must_use]
    pub fn component_ref(mut self, v: String) -> Self {
        self.component_ref = Some(v);
        self
    }

    #[must_use]
    pub fn component_id(mut self, v: String) -> Self {
        self.component_id = Some(v);
        self
    }

    #[must_use]
    pub fn constraints(mut self, v: BTreeMap<String, String>) -> Self {
        self.constraints = Some(v);
        self
    }

    pub fn build(self) -> Result<ComponentAuctionRequest> {
        Ok(ComponentAuctionRequest {
            component_ref: self
                .component_ref
                .ok_or_else(|| "component_ref is required".to_string())?,
            component_id: self
                .component_id
                .ok_or_else(|| "component_id is required".to_string())?,
            constraints: self.constraints.unwrap_or_default(),
        })
    }
}

/// A host response to a request to start a provider.
///
/// This acknowledgement confirms the host has enough capacity to
/// start the provider and that the provider is not already running on the host
///
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ProviderAuctionAck {
    /// The host ID of the "bidder" for this auction
    #[serde(default)]
    pub(crate) host_id: String,
    /// The original provider reference provided for the auction
    #[serde(default)]
    pub(crate) provider_ref: String,
    /// The unique component identifier that the auctioner can use for this provider
    #[serde(default)]
    pub(crate) provider_id: String,
    /// The constraints provided for the auction
    #[serde(default)]
    pub(crate) constraints: BTreeMap<String, String>,
}

impl ProviderAuctionAck {
    /// Get the Host ID for the provider auction acknowledgement
    #[must_use]
    pub fn host_id(&self) -> &str {
        self.host_id.as_ref()
    }

    /// Get the provider ref for the provider auction acknowledgement
    #[must_use]
    pub fn provider_ref(&self) -> &str {
        self.provider_ref.as_ref()
    }

    /// Get the provider ID for the provider auction acknowledgement
    #[must_use]
    pub fn provider_id(&self) -> &str {
        self.provider_id.as_ref()
    }

    /// Get the constraints for the provider auction acknowledgement
    #[must_use]
    pub fn constraints(&self) -> &BTreeMap<String, String> {
        &self.constraints
    }

    #[must_use]
    pub fn builder() -> ProviderAuctionAckBuilder {
        ProviderAuctionAckBuilder::default()
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
pub struct ProviderAuctionAckBuilder {
    host_id: Option<String>,
    provider_ref: Option<String>,
    provider_id: Option<String>,
    constraints: Option<BTreeMap<String, String>>,
}

impl ProviderAuctionAckBuilder {
    #[must_use]
    pub fn provider_ref(mut self, v: String) -> Self {
        self.provider_ref = Some(v);
        self
    }

    #[must_use]
    pub fn provider_id(mut self, v: String) -> Self {
        self.provider_id = Some(v);
        self
    }

    #[must_use]
    pub fn host_id(mut self, v: String) -> Self {
        self.host_id = Some(v);
        self
    }

    #[must_use]
    pub fn constraints(mut self, v: BTreeMap<String, String>) -> Self {
        self.constraints = Some(v);
        self
    }

    pub fn build(self) -> Result<ProviderAuctionAck> {
        Ok(ProviderAuctionAck {
            provider_ref: self
                .provider_ref
                .ok_or_else(|| "provider_ref is required".to_string())?,
            provider_id: self
                .provider_id
                .ok_or_else(|| "provider_id is required".to_string())?,
            host_id: self
                .host_id
                .ok_or_else(|| "host_id is required".to_string())?,
            constraints: self.constraints.unwrap_or_default(),
        })
    }
}

/// A request to locate a suitable host for a capability provider.
///
/// The provider's unique identity is used to rule out hosts on which the
/// provider is already running.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderAuctionRequest {
    /// The image reference, file or OCI, for this provider.
    #[serde(default)]
    pub(crate) provider_ref: String,

    /// The unique identifier to be used for this provider. The host will ensure
    /// that no other provider with the same ID is running on the host
    pub(crate) provider_id: String,

    /// The set of constraints that must match the labels of a suitable target host
    pub(crate) constraints: BTreeMap<String, String>,
}

impl ProviderAuctionRequest {
    /// Get the provider ref for the auction request
    #[must_use]
    pub fn provider_ref(&self) -> &str {
        self.provider_ref.as_ref()
    }

    /// Get the provider ID for the auction request
    #[must_use]
    pub fn provider_id(&self) -> &str {
        self.provider_id.as_ref()
    }

    /// Get the constraints acknowledged by the auction request
    #[must_use]
    pub fn constraints(&self) -> &BTreeMap<String, String> {
        &self.constraints
    }

    /// Build a new [`ProviderAuctionRequest`]
    #[must_use]
    pub fn builder() -> ProviderAuctionRequestBuilder {
        ProviderAuctionRequestBuilder::default()
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
pub struct ProviderAuctionRequestBuilder {
    provider_ref: Option<String>,
    provider_id: Option<String>,
    constraints: Option<BTreeMap<String, String>>,
}

impl ProviderAuctionRequestBuilder {
    #[must_use]
    pub fn provider_ref(mut self, v: String) -> Self {
        self.provider_ref = Some(v);
        self
    }

    #[must_use]
    pub fn provider_id(mut self, v: String) -> Self {
        self.provider_id = Some(v);
        self
    }

    #[must_use]
    pub fn constraints(mut self, v: BTreeMap<String, String>) -> Self {
        self.constraints = Some(v);
        self
    }

    pub fn build(self) -> Result<ProviderAuctionRequest> {
        Ok(ProviderAuctionRequest {
            provider_ref: self
                .provider_ref
                .ok_or_else(|| "provider_ref is required".to_string())?,
            provider_id: self
                .provider_id
                .ok_or_else(|| "provider_id is required".to_string())?,
            constraints: self.constraints.unwrap_or_default(),
        })
    }
}

/// A request to remove a set of interfaces linked between two components.
/// This will also force deletion of import interface configuration on the source component
/// and export interface configuration on the target component.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct DeleteInterfacesLinkRequest {
    /// The source component's identifier.
    pub(crate) source_id: String,

    /// Name of the link. Not providing this is equivalent to specifying Some("default")
    #[serde(default = "default_link_name")]
    pub(crate) name: String,

    /// WIT namespace of the link, e.g. `wasi` in `wasi:keyvalue/readwrite.get`
    pub(crate) wit_namespace: String,

    /// WIT package of the link, e.g. `keyvalue` in `wasi:keyvalue/readwrite.get`
    pub(crate) wit_package: String,
}

impl DeleteInterfacesLinkRequest {
    /// Get the source (component/provider) ID for delete request
    #[must_use]
    pub fn source_id(&self) -> &str {
        self.source_id.as_ref()
    }

    /// Get the link name for the link deletion request(or "default")
    #[must_use]
    pub fn link_name(&self) -> &str {
        self.name.as_ref()
    }

    /// Get the WIT namespace relevant to the link deletion request
    #[must_use]
    pub fn wit_namespace(&self) -> &str {
        self.wit_namespace.as_ref()
    }

    /// Get the WIT package relevant to the link deletion request
    #[must_use]
    pub fn wit_package(&self) -> &str {
        self.wit_package.as_ref()
    }

    #[must_use]
    pub fn builder() -> DeleteInterfacesLinkRequestBuilder {
        DeleteInterfacesLinkRequestBuilder::default()
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
pub struct DeleteInterfacesLinkRequestBuilder {
    source_id: Option<String>,
    name: Option<String>,
    wit_namespace: Option<String>,
    wit_package: Option<String>,
    wit_interfaces: Option<Vec<String>>,
}

impl DeleteInterfacesLinkRequestBuilder {
    pub fn source_id(mut self, v: String) -> Self {
        self.source_id = Some(v);
        self
    }

    pub fn name(mut self, v: String) -> Self {
        self.name = Some(v);
        self
    }

    pub fn wit_namespace(mut self, v: String) -> Self {
        self.wit_namespace = Some(v);
        self
    }

    pub fn wit_package(mut self, v: String) -> Self {
        self.wit_package = Some(v);
        self
    }

    pub fn build(self) -> Result<DeleteInterfacesLinkRequest> {
        Ok(DeleteInterfacesLinkRequest {
            source_id: self
                .source_id
                .ok_or_else(|| "source_id is required".to_string())?,
            name: self.name.ok_or_else(|| "name is required".to_string())?,
            wit_namespace: self
                .wit_namespace
                .ok_or_else(|| "wit_namespace is required".to_string())?,
            wit_package: self
                .wit_package
                .ok_or_else(|| "wit_package is required".to_string())?,
        })
    }
}

/// Helper function to provide a default link name
fn default_link_name() -> String {
    "default".to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ComponentAuctionAck, ComponentAuctionRequest, DeleteInterfaceLinkDefinitionRequest,
        ProviderAuctionAck, ProviderAuctionRequest,
    };

    #[test]
    fn component_auction_ack_builder() {
        assert_eq!(
            ComponentAuctionAck {
                component_ref: "component_ref".into(),
                component_id: "component_id".into(),
                host_id: "host_id".into(),
                constraints: BTreeMap::from([("a".into(), "b".into())])
            },
            ComponentAuctionAck::builder()
                .component_ref("component_ref".into())
                .component_id("component_id".into())
                .host_id("host_id".into())
                .constraints(BTreeMap::from([("a".into(), "b".into())]))
                .build()
                .unwrap()
        )
    }

    #[test]
    fn component_auction_request_builder() {
        assert_eq!(
            ComponentAuctionRequest {
                component_ref: "component_ref".into(),
                component_id: "component_id".into(),
                constraints: BTreeMap::from([("a".into(), "b".into())])
            },
            ComponentAuctionRequest::builder()
                .component_ref("component_ref".into())
                .component_id("component_id".into())
                .constraints(BTreeMap::from([("a".into(), "b".into())]))
                .build()
                .unwrap()
        )
    }

    #[test]
    fn provider_auction_ack_builder() {
        assert_eq!(
            ProviderAuctionAck {
                provider_ref: "provider_ref".into(),
                provider_id: "provider_id".into(),
                host_id: "host_id".into(),
                constraints: BTreeMap::from([("a".into(), "b".into())])
            },
            ProviderAuctionAck::builder()
                .provider_ref("provider_ref".into())
                .provider_id("provider_id".into())
                .host_id("host_id".into())
                .constraints(BTreeMap::from([("a".into(), "b".into())]))
                .build()
                .unwrap()
        )
    }

    #[test]
    fn provider_auction_request_builder() {
        assert_eq!(
            ProviderAuctionRequest {
                provider_ref: "provider_ref".into(),
                provider_id: "provider_id".into(),
                constraints: BTreeMap::from([("a".into(), "b".into())])
            },
            ProviderAuctionRequest::builder()
                .provider_ref("provider_ref".into())
                .provider_id("provider_id".into())
                .constraints(BTreeMap::from([("a".into(), "b".into())]))
                .build()
                .unwrap()
        )
    }

    #[test]
    fn delete_interface_link_definition_request_builder() {
        assert_eq!(
            DeleteInterfaceLinkDefinitionRequest {
                source_id: "source_id".into(),
                name: "name".into(),
                wit_namespace: "wit_namespace".into(),
                wit_package: "wit_package".into(),
            },
            DeleteInterfaceLinkDefinitionRequest::builder()
                .source_id("source_id".into())
                .name("name".into())
                .wit_namespace("wit_namespace".into())
                .wit_package("wit_package".into())
                .build()
                .unwrap()
        )
    }
}
