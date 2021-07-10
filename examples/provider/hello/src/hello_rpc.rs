use async_trait::async_trait;
use wasmbus_rpc::{
    context::Context,
    core::{HealthCheckRequest, HealthCheckResponse},
    provider::prelude::*,
    MessageDispatch, RpcError,
};
use wasmcloud_example_hello::{Hello, HelloReceiver};

/// Hello provider implementation
#[derive(Debug, Default)]
pub struct HelloProvider {}

/// Implementation of hello provider
#[async_trait]
impl Hello for HelloProvider {
    /// for any input string SSS,  return "Hello SSS"
    async fn say_hello(&self, _ctx: &'_ Context<'_>, arg: &String) -> Result<String, RpcError> {
        Ok(format!("Hello {}", arg))
    }
}

// TODO: derive macro should generate most of the following

impl HelloReceiver for HelloProvider {}
impl ProviderDispatch for HelloProvider {}

/// Capability Provider messages received from host
#[async_trait]
impl CapabilityProvider for HelloProvider {
    /// Perform health check. Called at regular intervals by host
    async fn health_request(
        &self,
        _ctx: &context::Context<'_>,
        _arg: &HealthCheckRequest,
    ) -> Result<HealthCheckResponse, RpcError> {
        println!("------ hello provider responding to health check");
        Ok(HealthCheckResponse {
            healthy: true,
            message: None,
        })
    }
}

#[async_trait]
impl MessageDispatch for HelloProvider {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: Message<'_>,
    ) -> Result<Message<'static>, RpcError> {
        // this would iterate through Traits, but there's only one here
        let resp = HelloReceiver::dispatch(self, ctx, &&message).await?;
        Ok(resp)
    }
}
