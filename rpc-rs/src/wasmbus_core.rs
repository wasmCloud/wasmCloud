// This file is generated automatically using wasmcloud-weld and smithy model definitions
//
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

/// Capability contract id, e.g. 'wasmcloud:httpserver'
pub type CapabilityContractId = String;

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

/// Link definition for an actor
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkDefinition {
    pub values: LinkSettings,
    /// provider public key
    pub provider_id: String,
    /// link name
    pub link_name: String,
    /// actor public key
    pub actor_id: String,
    /// contract id
    pub contract_id: String,
}

/// Settings associated with an actor-provider link
pub type LinkSettings = std::collections::HashMap<String, String>;

/// a protocol defines the semantics
/// of how a client and server communicate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Wasmbus {
    /// indicates this service's operations are handled by an provider (default false)
    #[serde(rename = "providerReceive")]
    #[serde(default)]
    pub provider_receive: bool,
    /// capability id such as "wasmbus:httpserver"
    /// always required for providerReceive, but optional for actorReceive
    #[serde(rename = "contractId")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<CapabilityContractId>,
    /// indicates this service's operations are handled by an actor (default false)
    #[serde(rename = "actorReceive")]
    #[serde(default)]
    pub actor_receive: bool,
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

/// CapabilityProvider service handles link + health-check messages from host
/// (need to finalize Link apis)
/// wasmbus.providerReceive
#[async_trait]
pub trait CapabilityProvider {
    /// Perform health check. Called at regular intervals by host
    async fn health_request(
        &self,
        ctx: &context::Context<'_>,
        arg: &HealthCheckRequest,
    ) -> Result<HealthCheckResponse, RpcError>;
}

/// CapabilityProviderReceiver receives messages defined in the CapabilityProvider service trait
/// CapabilityProvider service handles link + health-check messages from host
/// (need to finalize Link apis)
#[async_trait]
pub trait CapabilityProviderReceiver: MessageDispatch + CapabilityProvider {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: &Message<'_>,
    ) -> Result<Message<'static>, RpcError> {
        match message.method {
            "HealthRequest" => {
                let value: HealthCheckRequest = deserialize(message.arg.as_ref())?;
                let resp = CapabilityProvider::health_request(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "CapabilityProvider.HealthRequest",
                    arg: buf,
                })
            }
            _ => Err(RpcError::MethodNotHandled(format!(
                "CapabilityProvider::{}",
                message.method
            ))),
        }
    }
}

/// CapabilityProviderSender sends messages to a CapabilityProvider service
/// CapabilityProvider service handles link + health-check messages from host
/// (need to finalize Link apis)
#[derive(Debug)]
pub struct CapabilityProviderSender<T> {
    transport: T,
    config: client::SendConfig,
}

impl<T: Transport> CapabilityProviderSender<T> {
    pub fn new(config: client::SendConfig, transport: T) -> Self {
        CapabilityProviderSender { transport, config }
    }
}

#[async_trait]
impl<T: Transport + std::marker::Sync + std::marker::Send> CapabilityProvider
    for CapabilityProviderSender<T>
{
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
