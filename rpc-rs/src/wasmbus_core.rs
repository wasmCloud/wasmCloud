// This file is generated automatically using wasmcloud-weld and smithy model definitions
//

#![allow(clippy::ptr_arg)]
#[allow(unused_imports)]
use crate::{
    client, context, deserialize, serialize, Message, MessageDispatch, RpcError, Transport,
};
#[allow(unused_imports)]
use async_trait::async_trait;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use std::borrow::Cow;

pub const SMITHY_VERSION: &str = "1.0";

/// List of linked actors for a provider
pub type ActorLinks = Vec<LinkDefinition>;

/// health check request parameter
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HealthCheckRequest {}

/// Return value from actors and providers for health check status
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    /// A flag that indicates the the actor is healthy
    #[serde(default)]
    pub healthy: bool,
    /// A message containing additional information about the actors health
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// initialization data for a capability provider
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostData {
    pub env_values: HostEnvValues,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub lattice_rpc_prefix: String,
    #[serde(default)]
    pub lattice_rpc_url: String,
    #[serde(default)]
    pub lattice_rpc_user_jwt: String,
    #[serde(default)]
    pub lattice_rpc_user_seed: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub provider_key: String,
}

/// Environment settings for initializing a capability provider
pub type HostEnvValues = std::collections::HashMap<String, String>;

/// RPC message to capability provider
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Invocation {
    #[serde(default)]
    pub encoded_claims: String,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub id: String,
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub msg: Vec<u8>,
    #[serde(default)]
    pub operation: String,
    pub origin: WasmCloudEntity,
    pub target: WasmCloudEntity,
}

/// Response to an invocation
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvocationResponse {
    /// optional error message
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// id connecting this response to the invocation
    #[serde(default)]
    pub invocation_id: String,
    /// serialize response message
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub msg: Vec<u8>,
}

/// Link definition for an actor
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinkDefinition {
    /// actor public key
    #[serde(default)]
    pub actor_id: String,
    /// contract id
    #[serde(default)]
    pub contract_id: String,
    /// link name
    #[serde(default)]
    pub link_name: String,
    /// provider public key
    #[serde(default)]
    pub provider_id: String,
    pub values: LinkSettings,
}

/// Settings associated with an actor-provider link
pub type LinkSettings = std::collections::HashMap<String, String>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WasmCloudEntity {
    pub contract_id: crate::model::CapabilityContractId,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub public_key: String,
}

/// Actor service
/// wasmbus.actorReceive
#[async_trait]
pub trait Actor {
    /// Perform health check. Called at regular intervals by host
    async fn health_request(
        &self,
        ctx: &context::Context<'_>,
        arg: &HealthCheckRequest,
    ) -> Result<HealthCheckResponse, RpcError>;
}

/// ActorReceiver receives messages defined in the Actor service trait
/// Actor service
#[async_trait]
pub trait ActorReceiver: MessageDispatch + Actor {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: &Message<'_>,
    ) -> Result<Message<'_>, RpcError> {
        match message.method {
            "HealthRequest" => {
                let value: HealthCheckRequest = deserialize(message.arg.as_ref())?;
                let resp = Actor::health_request(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "Actor.HealthRequest",
                    arg: buf,
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
#[derive(Debug)]
pub struct ActorSender<T> {
    transport: T,
    config: client::SendConfig,
}

impl<T: Transport> ActorSender<T> {
    pub fn new(config: client::SendConfig, transport: T) -> Self {
        ActorSender { transport, config }
    }
}

#[async_trait]
impl<T: Transport + std::marker::Sync + std::marker::Send> Actor for ActorSender<T> {
    #[allow(unused)]
    /// Perform health check. Called at regular intervals by host
    async fn health_request(
        &self,
        ctx: &context::Context<'_>,
        arg: &HealthCheckRequest,
    ) -> Result<HealthCheckResponse, RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "HealthRequest",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        let value = deserialize(resp.arg.as_ref())?;
        Ok(value)
    }
}
