// This file is generated automatically using wasmcloud-weld and smithy model definitions
//
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

/// Headers is a list of http headers
pub type Headers = std::collections::HashMap<String, String>;

/// HttpRequest contains data sent to actor about the http request
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct HttpRequest {
    pub path: String,
    pub header: Headers,
    #[serde(rename = "queryString")]
    pub query_string: String,
    #[serde(with = "serde_bytes")]
    pub body: Vec<u8>,
    pub method: String,
}

/// HttpResponse contains the actor's response to return to the http client
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpResponse {
    #[serde(with = "serde_bytes")]
    pub body: Vec<u8>,
    /// statusCode should be 200 if the request was correctly handled
    #[serde(rename = "statusCode")]
    pub status_code: u32,
    pub status: String,
    pub header: Headers,
}

/// HttpServer is the contract to be implemented by actor
/// @direction(actorReceiver)
#[async_trait]
pub trait HttpServer {
    async fn handle_request(
        &self,
        ctx: &context::Context<'_>,
        arg: &HttpRequest,
    ) -> Result<HttpResponse, RpcError>;
}

/// HttpServerReceiver receives messages defined in the HttpServer service trait
/// HttpServer is the contract to be implemented by actor
/// @direction(actorReceiver)
#[async_trait]
pub trait HttpServerReceiver: MessageDispatch + HttpServer {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: &Message<'_>,
    ) -> Result<Message<'static>, RpcError> {
        match message.method {
            "HandleRequest" => {
                let value: HttpRequest = deserialize(message.arg.as_ref())?;
                let resp = HttpServer::handle_request(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "HttpServer.HandleRequest",
                    arg: buf,
                })
            }
            _ => Err(RpcError::MethodNotHandled(format!(
                "HttpServer::{}",
                message.method
            ))),
        }
    }
}

/// HttpServerSender sends messages to a HttpServer service
/// HttpServer is the contract to be implemented by actor
/// @direction(actorReceiver)
#[derive(Debug)]
pub struct HttpServerSender<T> {
    transport: T,
    config: client::SendConfig,
}

impl<T: Transport> HttpServerSender<T> {
    pub fn new(config: client::SendConfig, transport: T) -> Self {
        HttpServerSender { transport, config }
    }
}

#[async_trait]
impl<T: Transport + std::marker::Sync + std::marker::Send> HttpServer for HttpServerSender<T> {
    #[allow(unused)]
    async fn handle_request(
        &self,
        ctx: &context::Context<'_>,
        arg: &HttpRequest,
    ) -> Result<HttpResponse, RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "HandleRequest",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        let value = deserialize(resp.arg.as_ref())?;
        Ok(value)
    }
}
