use crate::HelloProvider;
use wasmbus_rpc::core::{HealthCheckRequest, HealthCheckResponse};
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_example_hello::{Hello, HelloReceiver};

/// Implementation of hello provider
#[async_trait]
impl Hello for HelloProvider {
    /// for any input string SSS,  return "Hello SSS"
    async fn say_hello(
        &self,
        _ctx: &'_ context::Context<'_>,
        arg: &String,
    ) -> Result<String, RpcError> {
        Ok(format!("Hello {}", arg))
    }
}

/// Your provider can handle any of these methods
/// to receive notification of new actor links, deleted links,
/// and for handling health check.
/// The default handlers are implemented in the trait ProviderHandler.
impl ProviderHandler for HelloProvider {
    /// Perform health check. Called at regular intervals by host
    fn health_request(&self, _arg: &HealthCheckRequest) -> Result<HealthCheckResponse, RpcError> {
        Ok(HealthCheckResponse {
            healthy: true,
            message: None,
        })
    }

    /*
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    /// This message is idempotent - provider must be able to handle
    /// duplicates
    fn put_link(&self, ld: &LinkDefinition) -> Result<bool, RpcError> {
        Ok(true)
    }
     */

    /*
    /// Notify the provider that the link is dropped
    fn delete_link(&self, actor_id: &str) {}
     */

    /*
    /// Handle system shutdown message
    fn shutdown(&self) -> Result<(), Infallible> {
        Ok(())
    }
     */
}

// TODO: derive macro should generate most of the following

impl HelloReceiver for HelloProvider {}
impl ProviderDispatch for HelloProvider {}

#[async_trait]
impl MessageDispatch for HelloProvider {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: Message<'_>,
    ) -> Result<Message<'_>, RpcError> {
        // this would iterate through Traits, but there's only one here
        let resp = HelloReceiver::dispatch(self, ctx, &message).await?;
        Ok(resp)
    }
}
