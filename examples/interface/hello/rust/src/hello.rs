// This file is generated automatically using wasmcloud-weld and smithy model definitions
//

#![allow(clippy::ptr_arg)]
#[allow(unused_imports)]
use async_trait::async_trait;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use std::borrow::Cow;
#[allow(unused_imports)]
use wasmbus_rpc::{
    client, context, deserialize, serialize, Message, MessageDispatch, RpcError, Transport,
};

pub const SMITHY_VERSION: &str = "1.0";

/// Simple service that responds to a message
/// wasmbus.contractId: wasmcloud:example:hello
/// wasmbus.providerReceive
/// wasmbus.actorReceive
#[async_trait]
pub trait Hello {
    /// Send a string message
    /// .Response is "Hello " + input message
    async fn say_hello(&self, ctx: &context::Context<'_>, arg: &String)
        -> Result<String, RpcError>;
}

/// HelloReceiver receives messages defined in the Hello service trait
/// Simple service that responds to a message
#[async_trait]
pub trait HelloReceiver: MessageDispatch + Hello {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: &Message<'_>,
    ) -> Result<Message<'static>, RpcError> {
        match message.method {
            "SayHello" => {
                let value: String = deserialize(message.arg.as_ref())?;
                let resp = Hello::say_hello(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "Hello.SayHello",
                    arg: buf,
                })
            }
            _ => Err(RpcError::MethodNotHandled(format!(
                "Hello::{}",
                message.method
            ))),
        }
    }
}

/// HelloSender sends messages to a Hello service
/// Simple service that responds to a message
#[derive(Debug)]
pub struct HelloSender<T> {
    transport: T,
    config: client::SendConfig,
}

impl<T: Transport> HelloSender<T> {
    pub fn new(config: client::SendConfig, transport: T) -> Self {
        HelloSender { transport, config }
    }
}

#[async_trait]
impl<T: Transport + std::marker::Sync + std::marker::Send> Hello for HelloSender<T> {
    #[allow(unused)]
    /// Send a string message
    /// .Response is "Hello " + input message
    async fn say_hello(
        &self,
        ctx: &context::Context<'_>,
        arg: &String,
    ) -> Result<String, RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "SayHello",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        let value = deserialize(resp.arg.as_ref())?;
        Ok(value)
    }
}
