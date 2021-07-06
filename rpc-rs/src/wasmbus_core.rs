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

/// Capability contract id, e.g. 'wasmcloud:httpserver'
pub type CapabilityContractId = String;

/// health check request parameter
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckRequest {}

/// Return value from actors and providers for health check status
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    /// A flag that indicates the the actor is healthy
    pub healthy: bool,
    /// A message containing additional information about the actors health
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Capability Provider messages received from host
/// @direction(providerReceiver)
#[async_trait]
pub trait CapabilityProvider {
    /// instruction to capability provider to bind actor
    async fn bind_actor(&self, ctx: &context::Context<'_>, arg: &String) -> Result<(), RpcError>;
    /// instruction to capability provider to remove actor actor
    async fn remove_actor(&self, ctx: &context::Context<'_>, arg: &String) -> Result<(), RpcError>;
    /// Perform health check. Called at regular intervals by host
    async fn health_request(
        &self,
        ctx: &context::Context<'_>,
        arg: &HealthCheckRequest,
    ) -> Result<HealthCheckResponse, RpcError>;
}

/// CapabilityProviderReceiver receives messages defined in the CapabilityProvider service trait
/// Capability Provider messages received from host
/// @direction(providerReceiver)
#[async_trait]
pub trait CapabilityProviderReceiver: MessageDispatch + CapabilityProvider {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: &Message<'_>,
    ) -> Result<Message<'static>, RpcError> {
        match message.method {
            "BindActor" => {
                let value: String = deserialize(message.arg.as_ref())?;
                let resp = CapabilityProvider::bind_actor(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "CapabilityProvider.BindActor",
                    arg: buf,
                })
            }
            "RemoveActor" => {
                let value: String = deserialize(message.arg.as_ref())?;
                let resp = CapabilityProvider::remove_actor(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "CapabilityProvider.RemoveActor",
                    arg: buf,
                })
            }
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
/// Capability Provider messages received from host
/// @direction(providerReceiver)
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
    /// instruction to capability provider to bind actor
    async fn bind_actor(&self, ctx: &context::Context<'_>, arg: &String) -> Result<(), RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "BindActor",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        Ok(())
    }
    #[allow(unused)]
    /// instruction to capability provider to remove actor actor
    async fn remove_actor(&self, ctx: &context::Context<'_>, arg: &String) -> Result<(), RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "RemoveActor",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        Ok(())
    }
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

/// Actor service
/// @direction(actorReceiver)
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
/// @direction(actorReceiver)
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
/// @direction(actorReceiver)
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
