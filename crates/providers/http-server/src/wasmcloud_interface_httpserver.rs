use std::{borrow::Cow, string::ToString};

use serde::{Deserialize, Serialize};
use wasmbus_rpc::{
    common::{Context, Message, MessageDispatch, Transport},
    error::{RpcError, RpcResult},
};
#[allow(dead_code)]
pub const SMITHY_VERSION: &str = "1.0";
/// map data structure for holding http headers
///
pub type HeaderMap = std::collections::HashMap<String, HeaderValues>;
#[doc(hidden)]
#[allow(unused_mut)]
pub fn encode_header_map<W: wasmbus_rpc::cbor::Write>(
    mut e: &mut wasmbus_rpc::cbor::Encoder<W>,
    val: &HeaderMap,
) -> RpcResult<()>
where
    <W as wasmbus_rpc::cbor::Write>::Error: std::fmt::Display,
{
    e.map(val.len() as u64)?;
    for (k, v) in val {
        e.str(k)?;
        encode_header_values(e, v)?;
    }
    Ok(())
}
#[doc(hidden)]
pub fn decode_header_map(d: &mut wasmbus_rpc::cbor::Decoder<'_>) -> Result<HeaderMap, RpcError> {
    let __result = {
        {
            let map_len = d.fixed_map()? as usize;
            let mut m: std::collections::HashMap<String, HeaderValues> =
                std::collections::HashMap::with_capacity(map_len);
            for _ in 0..map_len {
                let k = d.str()?.to_string();
                let v = decode_header_values(d).map_err(|e| {
                    format!("decoding 'org.wasmcloud.interface.httpserver#HeaderValues': {e}",)
                })?;
                m.insert(k, v);
            }
            m
        }
    };
    Ok(__result)
}
pub type HeaderValues = Vec<String>;
#[doc(hidden)]
#[allow(unused_mut)]
pub fn encode_header_values<W: wasmbus_rpc::cbor::Write>(
    mut e: &mut wasmbus_rpc::cbor::Encoder<W>,
    val: &HeaderValues,
) -> RpcResult<()>
where
    <W as wasmbus_rpc::cbor::Write>::Error: std::fmt::Display,
{
    e.array(val.len() as u64)?;
    for item in val.iter() {
        e.str(item)?;
    }
    Ok(())
}
#[doc(hidden)]
pub fn decode_header_values(
    d: &mut wasmbus_rpc::cbor::Decoder<'_>,
) -> Result<HeaderValues, RpcError> {
    let __result = {
        if let Some(n) = d.array()? {
            let mut arr: Vec<String> = Vec::with_capacity(n as usize);
            for _ in 0..(n as usize) {
                arr.push(d.str()?.to_string())
            }
            arr
        } else {
            let mut arr: Vec<String> = Vec::new();
            loop {
                match d.datatype() {
                    Err(_) => break,
                    Ok(wasmbus_rpc::cbor::Type::Break) => break,
                    Ok(_) => arr.push(d.str()?.to_string()),
                }
            }
            arr
        }
    };
    Ok(__result)
}
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

#[doc(hidden)]
#[allow(unused_mut)]
pub fn encode_http_request<W: wasmbus_rpc::cbor::Write>(
    mut e: &mut wasmbus_rpc::cbor::Encoder<W>,
    val: &HttpRequest,
) -> RpcResult<()>
where
    <W as wasmbus_rpc::cbor::Write>::Error: std::fmt::Display,
{
    e.array(5)?;
    e.str(&val.method)?;
    e.str(&val.path)?;
    e.str(&val.query_string)?;
    encode_header_map(e, &val.header)?;
    e.bytes(&val.body)?;
    Ok(())
}
#[doc(hidden)]
pub fn decode_http_request(
    d: &mut wasmbus_rpc::cbor::Decoder<'_>,
) -> Result<HttpRequest, RpcError> {
    let __result = {
        let mut method: Option<String> = None;
        let mut path: Option<String> = None;
        let mut query_string: Option<String> = None;
        let mut header: Option<HeaderMap> = None;
        let mut body: Option<Vec<u8>> = None;
        let is_array = match d.datatype()? {
            wasmbus_rpc::cbor::Type::Array => true,
            wasmbus_rpc::cbor::Type::Map => false,
            _ => {
                return Err(RpcError::Deser(
                    "decoding struct HttpRequest, expected array or map".to_string(),
                ));
            }
        };
        if is_array {
            let len = d.fixed_array()?;
            for __i in 0..(len as usize) {
                match __i {
                    0 => method = Some(d.str()?.to_string()),
                    1 => path = Some(d.str()?.to_string()),
                    2 => query_string = Some(d.str()?.to_string()),
                    3 => {
                        header = Some(decode_header_map(d).map_err(|e| {
                            format!("decoding 'org.wasmcloud.interface.httpserver#HeaderMap': {e}")
                        })?);
                    }
                    4 => body = Some(d.bytes()?.to_vec()),
                    _ => d.skip()?,
                }
            }
        } else {
            let len = d.fixed_map()?;
            for __i in 0..(len as usize) {
                match d.str()? {
                    "method" => method = Some(d.str()?.to_string()),
                    "path" => path = Some(d.str()?.to_string()),
                    "queryString" => query_string = Some(d.str()?.to_string()),
                    "header" => {
                        header = Some(decode_header_map(d).map_err(|e| {
                            format!("decoding 'org.wasmcloud.interface.httpserver#HeaderMap': {e}",)
                        })?);
                    }
                    "body" => body = Some(d.bytes()?.to_vec()),
                    _ => d.skip()?,
                }
            }
        }
        HttpRequest {
            method: if let Some(__x) = method {
                __x
            } else {
                return Err(RpcError::Deser(
                    "missing field HttpRequest.method (#0)".to_string(),
                ));
            },
            path: if let Some(__x) = path {
                __x
            } else {
                return Err(RpcError::Deser(
                    "missing field HttpRequest.path (#1)".to_string(),
                ));
            },
            query_string: if let Some(__x) = query_string {
                __x
            } else {
                return Err(RpcError::Deser(
                    "missing field HttpRequest.query_string (#2)".to_string(),
                ));
            },
            header: if let Some(__x) = header {
                __x
            } else {
                return Err(RpcError::Deser(
                    "missing field HttpRequest.header (#3)".to_string(),
                ));
            },
            body: if let Some(__x) = body {
                __x
            } else {
                return Err(RpcError::Deser(
                    "missing field HttpRequest.body (#4)".to_string(),
                ));
            },
        }
    };
    Ok(__result)
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

#[doc(hidden)]
#[allow(unused_mut)]
pub fn encode_http_response<W: wasmbus_rpc::cbor::Write>(
    mut e: &mut wasmbus_rpc::cbor::Encoder<W>,
    val: &HttpResponse,
) -> RpcResult<()>
where
    <W as wasmbus_rpc::cbor::Write>::Error: std::fmt::Display,
{
    e.array(3)?;
    e.u16(val.status_code)?;
    encode_header_map(e, &val.header)?;
    e.bytes(&val.body)?;
    Ok(())
}
#[doc(hidden)]
pub fn decode_http_response(
    d: &mut wasmbus_rpc::cbor::Decoder<'_>,
) -> Result<HttpResponse, RpcError> {
    let __result = {
        let mut status_code: Option<u16> = None;
        let mut header: Option<HeaderMap> = None;
        let mut body: Option<Vec<u8>> = None;
        let is_array = match d.datatype()? {
            wasmbus_rpc::cbor::Type::Array => true,
            wasmbus_rpc::cbor::Type::Map => false,
            _ => {
                return Err(RpcError::Deser(
                    "decoding struct HttpResponse, expected array or map".to_string(),
                ));
            }
        };
        if is_array {
            let len = d.fixed_array()?;
            for __i in 0..(len as usize) {
                match __i {
                    0 => status_code = Some(d.u16()?),
                    1 => {
                        header = Some(decode_header_map(d).map_err(|e| {
                            format!("decoding 'org.wasmcloud.interface.httpserver#HeaderMap': {e}")
                        })?);
                    }
                    2 => body = Some(d.bytes()?.to_vec()),
                    _ => d.skip()?,
                }
            }
        } else {
            let len = d.fixed_map()?;
            for __i in 0..(len as usize) {
                match d.str()? {
                    "statusCode" => status_code = Some(d.u16()?),
                    "header" => {
                        header = Some(decode_header_map(d).map_err(|e| {
                            format!("decoding 'org.wasmcloud.interface.httpserver#HeaderMap': {e}")
                        })?);
                    }
                    "body" => body = Some(d.bytes()?.to_vec()),
                    _ => d.skip()?,
                }
            }
        }
        HttpResponse {
            status_code: if let Some(__x) = status_code {
                __x
            } else {
                return Err(RpcError::Deser(
                    "missing field HttpResponse.status_code (#0)".to_string(),
                ));
            },
            header: if let Some(__x) = header {
                __x
            } else {
                return Err(RpcError::Deser(
                    "missing field HttpResponse.header (#1)".to_string(),
                ));
            },
            body: if let Some(__x) = body {
                __x
            } else {
                return Err(RpcError::Deser(
                    "missing field HttpResponse.body (#2)".to_string(),
                ));
            },
        }
    };
    Ok(__result)
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
            dyn ::core::future::Future<Output = RpcResult<HttpResponse>>
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
            dyn ::core::future::Future<Output = Result<Vec<u8>, RpcError>>
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
            if let Some(__ret) = None::<Result<Vec<u8>, RpcError>> {
                return __ret;
            }
            let __self = self;
            let message = message;
            let __ret: Result<Vec<u8>, RpcError> = {
                match message.method {
                    "HandleRequest" => {
                        let value: HttpRequest = wasmbus_rpc::common::deserialize(&message.arg)
                            .map_err(|e| RpcError::Deser(format!("'HttpRequest': {e}")))?;
                        let resp = HttpServer::handle_request(__self, ctx, &value).await?;
                        let buf = wasmbus_rpc::common::serialize(&resp)?;
                        Ok(buf)
                    }
                    _ => Err(RpcError::MethodNotHandled(format!(
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
impl<'send> HttpServerSender<wasmbus_rpc::provider::ProviderTransport<'send>> {
    /// Constructs a Sender using an actor's LinkDefinition,
    /// Uses the provider's HostBridge for rpc
    pub fn for_actor(ld: &'send wasmbus_rpc::core::LinkDefinition) -> Self {
        Self {
            transport: wasmbus_rpc::provider::ProviderTransport::new(ld, None),
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
            dyn ::core::future::Future<Output = RpcResult<HttpResponse>>
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
            if let Some(__ret) = None::<RpcResult<HttpResponse>> {
                return __ret;
            }
            let __self = self;
            let __ret: RpcResult<HttpResponse> = {
                let buf = wasmbus_rpc::common::serialize(arg)?;
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
                let value: HttpResponse = wasmbus_rpc::common::deserialize(&resp)
                    .map_err(|e| RpcError::Deser(format!("'{e}': HttpResponse")))?;
                Ok(value)
            };
            #[allow(unreachable_code)]
            __ret
        })
    }
}
