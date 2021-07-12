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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckRequest {}

/// Return value from actors and providers for health check status
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    /// A flag that indicates the the actor is healthy
    pub healthy: bool,
    /// A message containing additional information about the actors health
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The response to an invocation
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostData {
    pub env_values: HostEnvValues,
    pub host_id: String,
    pub lattice_rpc_prefix: String,
    pub lattice_rpc_url: String,
    pub lattice_rpc_user_jwt: String,
    pub lattice_rpc_user_seed: String,
    pub link_name: String,
    pub provider_key: String,
}

pub type HostEnvValues = std::collections::HashMap<String, String>;

/// RPC message to capability provider
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invocation {
    pub encoded_class: String,
    pub host_id: String,
    pub id: String,
    #[serde(with = "serde_bytes")]
    pub msg: Vec<u8>,
    pub operation: String,
    pub origin: WasmCloudEntity,
    pub target: WasmCloudEntity,
}

/// Link definition for an actor
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkDefinition {
    /// actor public key
    pub actor_id: String,
    /// contract id
    pub contract_id: String,
    /// link name
    pub link_name: String,
    /// provider public key
    pub provider_id: String,
    pub values: LinkSettings,
}

/// Settings associated with an actor-provider link
pub type LinkSettings = std::collections::HashMap<String, String>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WasmCloudEntity {
    pub contract_id: crate::model::CapabilityContractId,
    pub link_name: String,
    pub public_key: String,
}

/// data sent via wasmbus
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct WasmbusData {}

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
    ) -> Result<Message<'static>, RpcError> {
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
