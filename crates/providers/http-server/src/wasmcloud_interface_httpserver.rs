use std::{borrow::Cow, string::ToString};

use serde::{Deserialize, Serialize};
use wasmcloud_provider_sdk::{
    Context, Message, MessageDispatch, Transport,
    error::{InvocationError, InvocationResult},
};
#[allow(dead_code)]
pub const SMITHY_VERSION: &str = "1.0";
/// map data structure for holding http headers
///
pub type HeaderMap = std::collections::HashMap<String, HeaderValues>;
pub type HeaderValues = Vec<String>;

/// HttpRequest contains data sent to actor about the http request
#[derive(Serialize, Deserialize, Debug)]
pub struct HttpRequest {
    /// HTTP method. One of: GET,POST,PUT,DELETE,HEAD,OPTIONS,CONNECT,PATCH,TRACE
    #[serde(default)]
    pub method: String,
    /// full request path
    #[serde(default)]
    pub path: String,
    /// query string. May be an empty string if there were no query parameters.
    #[serde(rename = "queryString")]
    #[serde(default)]
    pub query_string: String,
    /// map of request headers (string key, string value)
    pub header: HeaderMap,
    /// Request body as a byte array. May be empty.
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

/// HttpResponse contains the actor's response to return to the http client
#[derive(Serialize, Deserialize, Debug)]
pub struct HttpResponse {
    /// statusCode is a three-digit number, usually in the range 100-599,
    /// A value of 200 indicates success.
    #[serde(rename = "statusCode")]
    #[serde(default)]
    pub status_code: u16,
    /// Map of headers (string keys, list of values)
    pub header: HeaderMap,
    /// Body of response as a byte array. May be an empty array.
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

/// HttpServer is the contract to be implemented by actor
/// wasmbus.contractId: wasmcloud:httpserver
/// wasmbus.actorReceive
pub trait HttpServer {
    /// returns the capability contract id for this interface
    fn contract_id() -> &'static str {
        "wasmcloud:httpserver"
    }
    #[must_use]
    #[allow(clippy::type_complexity, clippy::type_repetition_in_bounds)]
    fn handle_request<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        ctx: &'life1 Context,
        arg: &'life2 HttpRequest,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = InvocationResult<HttpResponse>>
                + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait;
}
/// HttpServerReceiver receives messages defined in the HttpServer service trait
/// HttpServer is the contract to be implemented by actor
#[doc(hidden)]
pub trait HttpServerReceiver: MessageDispatch + HttpServer {
    #[must_use]
    #[allow(
        clippy::async_yields_async,
        clippy::let_unit_value,
        clippy::no_effect_underscore_binding,
        clippy::shadow_same,
        clippy::type_complexity,
        clippy::type_repetition_in_bounds,
        clippy::used_underscore_binding
    )]
    fn dispatch<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        ctx: &'life1 Context,
        message: Message<'life2>,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = Result<Vec<u8>, InvocationError>>
                + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: ::core::marker::Sync + 'async_trait,
    {
        Box::pin(async move {
            if let Some(__ret) = None::<Result<Vec<u8>, InvocationError>> {
                return __ret;
            }
            let __self = self;
            let message = message;
            let __ret: Result<Vec<u8>, InvocationError> = {
                match message.method {
                    "HandleRequest" => {
                        let value: HttpRequest =
                            wasmcloud_provider_sdk::deserialize(&message.arg)
                                .map_err(|e| InvocationError::Deser(format!("'HttpRequest': {e}")))?;
                        let resp = HttpServer::handle_request(__self, ctx, &value).await?;
                        let buf = wasmcloud_provider_sdk::serialize(&resp)?;
                        Ok(buf)
                    }
                    _ => Err(InvocationError::MethodNotHandled(format!(
                        "HttpServer::{}",
                        message.method
                    ))),
                }
            };
            #[allow(unreachable_code)]
            __ret
        })
    }
}
/// HttpServerSender sends messages to a HttpServer service
/// HttpServer is the contract to be implemented by actor
/// client for sending HttpServer messages
pub struct HttpServerSender<T: Transport> {
    transport: T,
}
impl<T: Transport> HttpServerSender<T> {
    /// Constructs a HttpServerSender with the specified transport
    pub fn via(transport: T) -> Self {
        Self { transport }
    }
    pub fn set_timeout(&self, interval: std::time::Duration) {
        self.transport.set_timeout(interval);
    }
}
#[cfg(not(target_arch = "wasm32"))]
impl<'send> HttpServerSender<wasmcloud_provider_sdk::provider::ProviderTransport<'send>> {
    /// Constructs a Sender using an actor's LinkDefinition,
    /// Uses the provider's HostBridge for rpc
    pub fn for_actor(ld: &'send wasmcloud_provider_sdk::core::LinkDefinition) -> Self {
        Self {
            transport: wasmcloud_provider_sdk::provider::ProviderTransport::new(ld, None),
        }
    }
}
impl<T: Transport + std::marker::Sync + std::marker::Send> HttpServer for HttpServerSender<T> {
    #[allow(unused)]
    #[allow(
        clippy::async_yields_async,
        clippy::let_unit_value,
        clippy::no_effect_underscore_binding,
        clippy::shadow_same,
        clippy::type_complexity,
        clippy::type_repetition_in_bounds,
        clippy::used_underscore_binding
    )]
    fn handle_request<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        ctx: &'life1 Context,
        arg: &'life2 HttpRequest,
    ) -> ::core::pin::Pin<
        Box<
            dyn ::core::future::Future<Output = InvocationResult<HttpResponse>>
                + ::core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            if let Some(__ret) = None::<InvocationResult<HttpResponse>> {
                return __ret;
            }
            let __self = self;
            let __ret: InvocationResult<HttpResponse> = {
                let buf = wasmcloud_provider_sdk::serialize(arg)?;
                let resp = __self
                    .transport
                    .send(
                        ctx,
                        Message {
                            method: "HttpServer.HandleRequest",
                            arg: Cow::Borrowed(&buf),
                        },
                        None,
                    )
                    .await?;
                let value: HttpResponse = wasmcloud_provider_sdk::deserialize(&resp)
                    .map_err(|e| InvocationError::Deser(format!("'{e}': HttpResponse")))?;
                Ok(value)
            };
            #[allow(unreachable_code)]
            __ret
        })
    }
}
