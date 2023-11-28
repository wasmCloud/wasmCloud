use std::collections::HashMap;

use anyhow::bail;
use serde::{Deserialize, Serialize};

/// One of a potential list of responses to an actor auction
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorAuctionAck {
    /// The original actor reference used for the auction
    #[serde(default)]
    pub actor_ref: String,
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
    /// The reference for this actor. Can be any one of the acceptable forms
    /// of uniquely identifying an actor.
    #[serde(default)]
    pub actor_ref: String,
    /// The set of constraints to which any candidate host must conform
    pub constraints: ConstraintMap,
}

pub type ConstraintMap = std::collections::HashMap<String, String>;

/// A summary description of an actor within a host inventory
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorDescription {
    /// Actor's 56-character unique ID
    #[serde(default)]
    pub id: String,
    /// Image reference for this actor, if applicable
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    /// The individual instances of this actor that are running
    pub instances: Vec<ActorInstance>,
    /// Name of this actor, if one exists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorInstance {
    /// The annotations that were used in the start request that produced
    /// this actor instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AnnotationMap>,
    /// Image reference for this actor, if applicable
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    /// This instance's unique ID (guid)
    #[serde(default)]
    pub instance_id: String,
    /// The revision number for this actor instance
    #[serde(default)]
    pub revision: i32,
    /// The maximum number of concurrent requests this instance can handle
    #[serde(default)]
    pub max_instances: u16,
}

pub type AnnotationMap = std::collections::HashMap<String, String>;

/// Standard response for control interface operations
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CtlOperationAck {
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub error: String,
}

/// A response containing the full list of known claims within the lattice
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetClaimsResponse {
    pub claims: Vec<HashMap<String, String>>,
}

/// The response returned when fetching the data for a single config key
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GetConfigKeyResponse {
    /// Whether or not the key was found
    pub found: bool,
    /// The value of the key, if found
    pub data: Vec<u8>,
}

/// A summary representation of a host
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Host {
    /// Comma-delimited list of valid cluster issuer public keys as known
    /// to this host
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_issuers: Option<String>,
    /// NATS server host used for the control interface
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctl_host: Option<String>,
    /// Human-friendly name for this host
    #[serde(default)]
    pub friendly_name: String,
    #[serde(default)]
    pub id: String,
    /// JetStream domain (if applicable) in use by this host
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub js_domain: Option<String>,
    /// Hash map of label-value pairs for this host
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<KeyValueMap>,
    /// Lattice prefix/ID used by the host
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lattice_prefix: Option<String>,
    /// NATS server host used for regular RPC
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_host: Option<String>,
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
/// a query
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostInventory {
    /// Actors running on this host.
    pub actors: Vec<ActorDescription>,
    /// The host's unique ID
    #[serde(default)]
    pub host_id: String,
    /// The host's cluster issuer public key
    #[serde(default)]
    pub issuer: String,
    /// The host's human-readable friendly name
    #[serde(default)]
    pub friendly_name: String,
    /// The host's labels
    pub labels: LabelsMap,
    /// Providers running on this host
    pub providers: ProviderDescriptions,
}

pub type KeyValueMap = std::collections::HashMap<String, String>;
pub type LabelsMap = std::collections::HashMap<String, String>;

/// A list of link definitions
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinkDefinitionList {
    pub links: Vec<LinkDefinition>,
}

/// One of a potential list of responses to a provider auction
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderAuctionAck {
    /// The host ID of the "bidder" for this auction
    #[serde(default)]
    pub host_id: String,
    /// The link name provided for the auction
    #[serde(default)]
    pub link_name: String,
    /// The original provider ref provided for the auction
    #[serde(default)]
    pub provider_ref: String,
    /// The constraints provided for the auction
    #[serde(default)]
    pub constraints: HashMap<String, String>,
}

/// A request to locate a suitable host for a capability provider. The
/// provider's unique identity (reference + link name) is used to rule
/// out sites on which the provider is already running.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderAuctionRequest {
    /// The set of constraints to which a suitable target host must conform
    pub constraints: ConstraintMap,
    /// The link name of the provider
    #[serde(default)]
    pub link_name: String,
    /// The reference for the provider. Can be any one of the accepted
    /// forms of uniquely identifying a provider
    #[serde(default)]
    pub provider_ref: String,
}

/// A summary description of a capability provider within a host inventory
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderDescription {
    /// The annotations that were used in the start request that produced
    /// this provider instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AnnotationMap>,
    /// Provider's unique 56-character ID
    #[serde(default)]
    pub id: String,
    /// Image reference for this provider, if applicable
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    /// Provider's contract ID
    #[serde(default)]
    pub contract_id: String,
    /// Provider's link name
    #[serde(default)]
    pub link_name: String,
    /// Name of the provider, if one exists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The revision of the provider
    #[serde(default)]
    pub revision: i32,
}

pub type ProviderDescriptions = Vec<ProviderDescription>;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RegistryCredential {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// If supplied, token authentication will be used for the registry
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// If supplied, username and password will be used for HTTP Basic authentication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The type of the registry (only "oci" is supported at this time")
    #[serde(rename = "registryType", default = "default_registry_type")]
    pub registry_type: String,
}

fn default_registry_type() -> String {
    "oci".to_string()
}

impl TryFrom<&RegistryCredential> for oci_distribution::secrets::RegistryAuth {
    type Error = anyhow::Error;

    fn try_from(cred: &RegistryCredential) -> Result<Self, Self::Error> {
        if cred.registry_type != "oci" {
            bail!("Only OCI registries are supported at this time");
        }

        match cred {
            RegistryCredential {
                username: Some(username),
                password: Some(password),
                ..
            } => Ok(oci_distribution::secrets::RegistryAuth::Basic(
                username.clone(),
                password.clone(),
            )),

            RegistryCredential {
                username: Some(username),
                password: None,
                token: Some(token),
                ..
            } => Ok(oci_distribution::secrets::RegistryAuth::Basic(
                username.clone(),
                token.clone(),
            )),
            _ => bail!("Invalid OCI registry credentials"),
        }
    }
}

/// A set of credentials to be used for fetching from specific registries
pub type RegistryCredentialMap = std::collections::HashMap<String, RegistryCredential>;

/// A request to remove a link definition and detach the relevant actor
/// from the given provider
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoveLinkDefinitionRequest {
    /// The actor's public key. This cannot be an image reference
    #[serde(default)]
    pub actor_id: String,
    /// The provider contract
    #[serde(default)]
    pub contract_id: String,
    /// The provider's link name
    #[serde(default)]
    pub link_name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScaleActorCommand {
    /// Image reference for the actor.
    #[serde(default)]
    pub actor_ref: String,
    /// Optional set of annotations used to describe the nature of this actor scale command. For
    /// example, autonomous agents may wish to "tag" scale requests as part of a given deployment
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AnnotationMap>,
    /// The maximum number of concurrent executing instances of this actor. Setting this to `0` will
    /// stop the actor.
    // NOTE: renaming to `count` lets us remain backwards compatible for a few minor versions
    #[serde(default, alias = "count", rename = "count")]
    pub max_instances: u16,
    /// Host ID on which to scale this actor
    #[serde(default)]
    pub host_id: String,
}

/// A command sent to a host requesting a capability provider be started with the
/// given link name and optional configuration.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartProviderCommand {
    /// Optional set of annotations used to describe the nature of this provider start command. For
    /// example, autonomous agents may wish to "tag" start requests as part of a given deployment
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AnnotationMap>,
    /// Optional provider configuration in the form of an opaque string. Many
    /// providers prefer base64-encoded JSON here, though that data should never
    /// exceed 500KB
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<String>,
    /// The host ID on which to start the provider
    #[serde(default)]
    pub host_id: String,
    /// The link name of the provider to be started
    #[serde(default)]
    pub link_name: String,
    /// The image reference of the provider to be started
    #[serde(default)]
    pub provider_ref: String,
}

/// A command sent to a host to request that instances of a given actor
/// be terminated on that host
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopActorCommand {
    /// Reference for this actor. Can be any of the means of uniquely identifying
    /// an actor
    #[serde(default)]
    pub actor_ref: String,
    /// Optional set of annotations used to describe the nature of this
    /// stop request. If supplied, the only instances of this actor with these
    /// annotations will be stopped
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AnnotationMap>,
    /// The ID of the target host
    #[serde(default)]
    pub host_id: String,
}

/// A command sent to request that the given host purge and stop
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopHostCommand {
    /// The ID of the target host
    #[serde(default)]
    pub host_id: String,
    /// An optional timeout, in seconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// A request to stop the given provider on the indicated host
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopProviderCommand {
    /// Optional set of annotations used to describe the nature of this
    /// stop request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AnnotationMap>,
    /// Contract ID of the capability provider
    #[serde(default)]
    pub contract_id: String,
    /// Host ID on which to stop the provider
    #[serde(default)]
    pub host_id: String,
    /// Link name for this provider
    #[serde(default)]
    pub link_name: String,
    /// Reference for the capability provider. Can be any of the forms of
    /// uniquely identifying a provider
    #[serde(default)]
    pub provider_ref: String,
}

/// A command instructing a specific host to perform a live update
/// on the indicated actor by supplying a new image reference. Note that
/// live updates are only possible through image references
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateActorCommand {
    /// The actor's 56-character unique ID
    #[serde(default)]
    pub actor_id: String,
    /// Optional set of annotations used to describe the nature of this
    /// update request. Only actor instances that have matching annotations
    /// will be upgraded, allowing for instance isolation by
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AnnotationMap>,
    /// The host ID of the host to perform the live update
    #[serde(default)]
    pub host_id: String,
    /// The new image reference of the upgraded version of this actor
    #[serde(default)]
    pub new_actor_ref: String,
}

// Below are copied structs to avoid depedency conflicts on wasmbus_rpc

// COPIED FROM https://github.com/wasmCloud/weld/blob/wasmbus-rpc-v0.13.0/rpc-rs/src/wasmbus_core.rs#L1176
/// Settings associated with an actor-provider link
pub type LinkSettings = std::collections::HashMap<String, String>;

// COPIED FROM https://github.com/wasmCloud/weld/blob/wasmbus-rpc-v0.13.0/rpc-rs/src/wasmbus_core.rs#L1042
/// Link definition for binding actor to provider
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinkDefinition {
    /// actor public key
    #[serde(default)]
    pub actor_id: String,
    /// provider public key
    #[serde(default)]
    pub provider_id: String,
    /// link name
    #[serde(default)]
    pub link_name: String,
    /// contract id
    #[serde(default)]
    pub contract_id: String,
    pub values: LinkSettings,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostLabel {
    pub key: String,
    pub value: String,
}
