use async_trait::async_trait;
use wasmbus_rpc::{context::Context, provider::prelude::*, MessageDispatch, RpcError};
use wasmcloud_example_hello::{Hello, HelloReceiver};

/// Hello provider implementation
#[derive(Clone, Debug, Default)]
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

#[async_trait]
impl MessageDispatch for HelloProvider {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: Message<'_>,
    ) -> Result<Message<'static>, RpcError> {
        // this would iterate through Traits, but there's only one here
        let resp = HelloReceiver::dispatch(self, ctx, &message).await?;
        Ok(resp)
    }
}
