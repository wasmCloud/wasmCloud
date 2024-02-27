use std::collections::HashMap;

use anyhow::bail;
use serde::{Deserialize, Serialize};

/// A control interface response that wraps a response payload, a success flag, and a message
/// with additional context if necessary.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CtlResponse<T> {
    /// Whether the request succeeded
    pub success: bool,
    /// A message with additional context about the response
    pub message: String,
    /// The response data, if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<T>,
}

impl<T> CtlResponse<T> {
    pub fn ok(response: T) -> Self {
        CtlResponse {
            success: true,
            message: "".to_string(),
            response: Some(response),
        }
    }
}

impl CtlResponse<()> {
    /// Helper function to return a successful response without
    /// a message or a payload.
    pub fn success() -> Self {
        CtlResponse {
            success: true,
            message: "".to_string(),
            response: None,
        }
    }

    /// Helper function to return an unsuccessful response with
    /// a message but no payload. Note that this implicitly is
    /// typing the inner payload as `()` for efficiency.
    pub fn error(message: &str) -> Self {
        CtlResponse {
            success: false,
            message: message.to_string(),
            response: None,
        }
    }
}

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

/// A summary description of an actor within a host inventory
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorDescription {
    /// The unique component identifier for this actor
    #[serde(default)]
    pub id: ComponentId,
    /// Image reference for this actor
    #[serde(default)]
    pub image_ref: String,
    /// Name of this actor, if one exists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The annotations that were used in the start request that produced
    /// this actor instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
    /// The revision number for this actor instance
    #[serde(default)]
    pub revision: i32,
    /// The maximum number of concurrent requests this instance can handle
    #[serde(default)]
    pub max_instances: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorInstance {
    /// The annotations that were used in the start request that produced
    /// this actor instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
    /// Image reference for this actor
    #[serde(default)]
    pub image_ref: String,
    /// This instance's unique ID (guid)
    #[serde(default)]
    pub instance_id: String,
    /// The revision number for this actor instance
    #[serde(default)]
    pub revision: i32,
    /// The maximum number of concurrent requests this instance can handle
    #[serde(default)]
    pub max_instances: u32,
}

/// A response containing the full list of known claims within the lattice
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetClaimsResponse {
    pub claims: Vec<HashMap<String, String>>,
}

/// A summary representation of a host
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Host {
    /// Comma-delimited list of valid cluster issuer public keys as known
    /// to this host
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_issuers: Option<String>,
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
    /// Actors running on this host.
    pub actors: Vec<ActorDescription>,
    /// Providers running on this host
    pub providers: Vec<ProviderDescription>,
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
    #[serde(default)]
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

/// A summary description of a capability provider within a host inventory
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderDescription {
    /// The annotations that were used in the start request that produced
    /// this provider instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
    /// Provider's unique identifier
    #[serde(default)]
    pub id: ComponentId,
    /// Image reference for this provider, if applicable
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    /// Name of the provider, if one exists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The revision of the provider
    #[serde(default)]
    pub revision: i32,
}

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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScaleActorCommand {
    /// Image reference for the actor.
    #[serde(default)]
    pub actor_ref: String,
    /// Unique identifier of the actor to scale.
    pub actor_id: ComponentId,
    /// Optional set of annotations used to describe the nature of this actor scale command. For
    /// example, autonomous agents may wish to "tag" scale requests as part of a given deployment
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
    /// The maximum number of concurrent executing instances of this actor. Setting this to `0` will
    /// stop the actor.
    // NOTE: renaming to `count` lets us remain backwards compatible for a few minor versions
    #[serde(default, alias = "count", rename = "count")]
    pub max_instances: u32,
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
    pub annotations: Option<HashMap<String, String>>,
    /// Unique identifier of the provider to start.
    pub provider_id: ComponentId,
    /// Optional provider configuration in the form of an opaque string. Many
    /// providers prefer base64-encoded JSON here, though that data should never
    /// exceed 500KB
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<String>,
    /// The host ID on which to start the provider
    #[serde(default)]
    pub host_id: String,
    /// The image reference of the provider to be started
    #[serde(default)]
    pub provider_ref: String,
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
    /// Host ID on which to stop the provider
    #[serde(default)]
    pub host_id: String,
    /// Unique identifier for the provider to stop.
    #[serde(default, alias = "provider_ref")]
    pub provider_id: ComponentId,
}

/// A command instructing a specific host to perform a live update
/// on the indicated actor by supplying a new image reference. Note that
/// live updates are only possible through image references
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateActorCommand {
    /// The actor's 56-character unique ID
    #[serde(default)]
    pub actor_id: ComponentId,
    /// Optional set of annotations used to describe the nature of this
    /// update request. Only actor instances that have matching annotations
    /// will be upgraded, allowing for instance isolation by
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
    /// The host ID of the host to perform the live update
    #[serde(default)]
    pub host_id: String,
    /// The new image reference of the upgraded version of this actor
    #[serde(default)]
    pub new_actor_ref: String,
}

/// Identifier of one or more entities on the lattice used for addressing. May take many forms, such as:
/// - actor public key
/// - provider public key
/// - opaque string
pub type LatticeTarget = String;

/// Identifier of a component which sends invocations on the lattice
pub type ComponentId = String;

/// Name of a link on the wasmCloud lattice
pub type LinkName = String;

/// WIT package for a given operation (ex. `keyvalue` in `wasi:keyvalue/readwrite.get`)
pub type WitPackage = String;

/// WIT namespace for a given operation (ex. `wasi` in `wasi:keyvalue/readwrite.get`)
pub type WitNamespace = String;

/// WIT interface for a given operation (ex. `readwrite` in `wasi:keyvalue/readwrite.get`)
pub type WitInterface = String;

/// The name of a known (possibly pre-created) configuration, normally used when creating
/// new interface links in order to configure one or both source/target
pub type KnownConfigName = String;

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
fn default_link_name() -> LinkName {
    "default".to_string()
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostLabel {
    pub key: String,
    pub value: String,
}
