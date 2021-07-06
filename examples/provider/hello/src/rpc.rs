use async_trait::async_trait;
use wasmbus_rpc::{context::Context, provider::prelude::*, MessageDispatch, RpcError};
use wasmcloud_example_hello::{Hello, HelloReceiver};

/// Hello provider implementation
//#[derive(Debug, Default, Provider)]
//#[services(Provider, HttpServer)]

#[derive(Debug, Default)]
pub struct HelloProvider {}

impl HelloReceiver for HelloProvider {}

// TODO: derive macro should generate this

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

#[async_trait]
impl Hello for HelloProvider {
    async fn say_hello(&self, _ctx: &'_ Context<'_>, arg: &String) -> Result<String, RpcError> {
        Ok(format!("Hello {}", arg))
    }
}
