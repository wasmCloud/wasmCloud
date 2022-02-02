// This file is generated automatically using wasmcloud/weld-codegen and smithy model definitions
//

#![allow(unused_imports, clippy::ptr_arg, clippy::needless_lifetimes)]
use crate::{
    deserialize, serialize, Context, Message, MessageDispatch, RpcError, RpcResult, SendOpts,
    Timestamp, Transport,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, io::Write, string::ToString};

pub const SMITHY_VERSION: &str = "1.0";

/// List of linked actors for a provider
pub type ActorLinks = Vec<LinkDefinition>;

pub type ClusterIssuerKey = String;

pub type ClusterIssuers = Vec<ClusterIssuerKey>;

/// health check request parameter
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthCheckRequest {}

/// Return value from actors and providers for health check status
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthCheckResponse {
    /// A flag that indicates the the actor is healthy
    #[serde(default)]
    pub healthy: bool,
    /// A message containing additional information about the actors health
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// initialization data for a capability provider
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostData {
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub lattice_rpc_prefix: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub lattice_rpc_user_jwt: String,
    #[serde(default)]
    pub lattice_rpc_user_seed: String,
    #[serde(default)]
    pub lattice_rpc_url: String,
    #[serde(default)]
    pub provider_key: String,
    #[serde(default)]
    pub invocation_seed: String,
    pub env_values: HostEnvValues,
    #[serde(default)]
    pub instance_id: String,
    /// initial list of links for provider
    pub link_definitions: ActorLinks,
    /// list of cluster issuers
    pub cluster_issuers: ClusterIssuers,
    /// Optional configuration JSON sent to a given link name of a provider
    /// without an actor context
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_json: Option<String>,
}

/// Environment settings for initializing a capability provider
pub type HostEnvValues = std::collections::HashMap<String, String>;

/// RPC message to capability provider
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Invocation {
    pub origin: WasmCloudEntity,
    pub target: WasmCloudEntity,
    #[serde(default)]
    pub operation: String,
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub msg: Vec<u8>,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub encoded_claims: String,
    #[serde(default)]
    pub host_id: String,
}

/// Response to an invocation
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct InvocationResponse {
    /// serialize response message
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub msg: Vec<u8>,
    /// id connecting this response to the invocation
    #[serde(default)]
    pub invocation_id: String,
    /// optional error message
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

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

/// Settings associated with an actor-provider link
pub type LinkSettings = std::collections::HashMap<String, String>;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WasmCloudEntity {
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub link_name: String,
    pub contract_id: crate::model::CapabilityContractId,
}

/// Actor service
/// wasmbus.actorReceive
#[async_trait]
pub trait Actor {
    /// Perform health check. Called at regular intervals by host
    async fn health_request(
        &self,
        ctx: &Context,
        arg: &HealthCheckRequest,
    ) -> RpcResult<HealthCheckResponse>;
}

/// ActorReceiver receives messages defined in the Actor service trait
/// Actor service
#[doc(hidden)]
#[async_trait]
pub trait ActorReceiver: MessageDispatch + Actor {
    async fn dispatch(&self, ctx: &Context, message: &Message<'_>) -> RpcResult<Message<'_>> {
        match message.method {
            "HealthRequest" => {
                let value: HealthCheckRequest = deserialize(message.arg.as_ref())
                    .map_err(|e| RpcError::Deser(format!("message '{}': {}", message.method, e)))?;
                let resp = Actor::health_request(self, ctx, &value).await?;
                let buf = serialize(&resp)?;
                Ok(Message {
                    method: "Actor.HealthRequest",
                    arg: Cow::Owned(buf),
                })
            }
            _ => Err(RpcError::MethodNotHandled(format!(
                "Actor::{}",
                message.method
            ))),
        }
    }
}

/// ActorSender sends messages to a Actor service
/// Actor service
/// client for sending Actor messages
#[derive(Debug)]
pub struct ActorSender<T: Transport> {
    transport: T,
}

impl<T: Transport> ActorSender<T> {
    /// Constructs a ActorSender with the specified transport
    pub fn via(transport: T) -> Self {
        Self { transport }
    }

    pub fn set_timeout(&self, interval: std::time::Duration) {
        self.transport.set_timeout(interval);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<'send> ActorSender<crate::provider::ProviderTransport<'send>> {
    /// Constructs a Sender using an actor's LinkDefinition,
    /// Uses the provider's HostBridge for rpc
    pub fn for_actor(ld: &'send crate::core::LinkDefinition) -> Self {
        Self {
            transport: crate::provider::ProviderTransport::new(ld, None),
        }
    }
}
#[cfg(target_arch = "wasm32")]
impl ActorSender<crate::actor::prelude::WasmHost> {
    /// Constructs a client for actor-to-actor messaging
    /// using the recipient actor's public key
    pub fn to_actor(actor_id: &str) -> Self {
        let transport = crate::actor::prelude::WasmHost::to_actor(actor_id.to_string()).unwrap();
        Self { transport }
    }
}
#[async_trait]
impl<T: Transport + std::marker::Sync + std::marker::Send> Actor for ActorSender<T> {
    #[allow(unused)]
    /// Perform health check. Called at regular intervals by host
    async fn health_request(
        &self,
        ctx: &Context,
        arg: &HealthCheckRequest,
    ) -> RpcResult<HealthCheckResponse> {
        let buf = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                Message {
                    method: "Actor.HealthRequest",
                    arg: Cow::Borrowed(&buf),
                },
                None,
            )
            .await?;
        let value = deserialize(&resp)
            .map_err(|e| RpcError::Deser(format!("response to {}: {}", "HealthRequest", e)))?;
        Ok(value)
    }
}
