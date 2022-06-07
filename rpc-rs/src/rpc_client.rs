#![cfg(not(target_arch = "wasm32"))]

use std::{
    convert::{TryFrom, TryInto},
    sync::Arc,
    time::Duration,
};

use nats_aflowt::{header::HeaderMap, Connection};
use ring::digest::{Context, SHA256};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value as JsonValue;
use tracing::{debug, error, instrument, trace};

#[cfg(all(feature = "chunkify", not(target_arch = "wasm32")))]
use crate::chunkify::chunkify_endpoint;
#[cfg(feature = "otel")]
use crate::otel::OtelHeaderInjector;
use crate::{
    common::Message,
    core::{Invocation, InvocationResponse, WasmCloudEntity},
    error::{RpcError, RpcResult},
};

pub(crate) const DEFAULT_RPC_TIMEOUT_MILLIS: Duration = Duration::from_millis(2000);
/// Amount of time to add to rpc timeout if chunkifying
pub(crate) const CHUNK_RPC_EXTRA_TIME: Duration = Duration::from_secs(13);

/// Send wasmbus rpc messages
///
/// The primary use of RpcClient is providers sending to actors,
/// however providers don't need to construct this - one is created
/// by HostBridge, which providers create during initialization.
///
/// This class is also used by wash and test tools for sending
/// rpc messages to actors and providers. Note that sending to
/// a provider requires an existing link definition, _and_,
/// the message needs to be signed by a valid cluster key.
///
/// This RpcClient does not subscribe to rpc topics.
/// To subscribe, use the nats client api directly.
///
#[derive(Clone)]
pub struct RpcClient {
    /// sync or async nats client
    client: NatsClientType,
    /// lattice rpc prefix
    lattice_prefix: String,
    /// secrets for signing invocations
    key: Arc<wascap::prelude::KeyPair>,
    /// host id for invocations
    host_id: String,
    /// timeout for rpc messages
    timeout: Option<Duration>,
}

#[derive(Clone)]
#[non_exhaustive]
pub(crate) enum NatsClientType {
    Async(crate::anats::Connection),
    #[cfg(feature = "async_rewrite")]
    AsyncRewrite(nats_experimental::Client),
    //#[cfg(feature = "chunkify")]
    //Sync(nats::Connection),
}

/// Returns the rpc topic (subject) name for sending to an actor or provider.
/// A provider entity must have the public_key and link_name fields filled in.
/// An actor entity must have a public_key and an empty link_name.
#[doc(hidden)]
pub fn rpc_topic(entity: &WasmCloudEntity, lattice_prefix: &str) -> String {
    if !entity.link_name.is_empty() {
        // provider target
        format!(
            "wasmbus.rpc.{}.{}.{}",
            lattice_prefix, entity.public_key, entity.link_name
        )
    } else {
        // actor target
        format!("wasmbus.rpc.{}.{}", lattice_prefix, entity.public_key)
    }
}

impl RpcClient {
    /// Constructs a new RpcClient with an async nats connection.
    /// parameters: async nats client, lattice rpc prefix (usually "default"),
    /// secret key for signing messages, host_id, and optional timeout.
    pub fn new(
        nats: crate::anats::Connection,
        lattice_prefix: &str,
        key: wascap::prelude::KeyPair,
        host_id: String,
        timeout: Option<Duration>,
    ) -> Self {
        Self::new_client(
            NatsClientType::Async(nats),
            lattice_prefix,
            key,
            host_id,
            timeout,
        )
    }

    /// Constructs a new RpcClient with a nats connection.
    /// parameters: nats client, lattice rpc prefix (usually "default"),
    /// secret key for signing messages, host_id, and optional timeout.
    pub(crate) fn new_client(
        nats: NatsClientType,
        lattice_prefix: &str,
        key: wascap::prelude::KeyPair,
        host_id: String,
        timeout: Option<Duration>,
    ) -> Self {
        RpcClient {
            client: nats,
            lattice_prefix: lattice_prefix.to_string(),
            key: Arc::new(key),
            host_id,
            timeout,
        }
    }

    /// convenience method for returning async client
    /// If the client is not the correct type, returns None
    #[cfg(feature = "async_rewrite")]
    pub fn get_async(&self) -> Option<nats_experimental::Client> {
        use std::borrow::Borrow;
        match self.client.borrow() {
            NatsClientType::AsyncRewrite(nc) => Some(nc.clone()),
            _ => None,
        }
    }

    /// convenience method for returning async client
    /// If the client is not the correct type, returns None
    #[cfg(not(feature = "async_rewrite"))]
    pub fn get_async(&self) -> Option<crate::anats::Connection> {
        use std::borrow::Borrow;
        #[allow(unreachable_patterns)]
        match self.client.borrow() {
            NatsClientType::Async(nats) => Some(nats.clone()),
            _ => None,
        }
    }

    /// returns the lattice prefix
    pub fn lattice_prefix(&self) -> &str {
        self.lattice_prefix.as_str()
    }

    /// Replace the default timeout with the specified value.
    /// If the parameter is None, unsets the default timeout
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// Send an rpc message using json-encoded data
    pub async fn send_json<Target, Arg, Resp>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        method: &str,
        data: JsonValue,
    ) -> RpcResult<JsonValue>
    where
        Arg: DeserializeOwned + Serialize,
        Resp: DeserializeOwned + Serialize,
        Target: Into<WasmCloudEntity>,
    {
        let msg = JsonMessage(method, data).try_into()?;
        let bytes = self.send(origin, target, msg).await?;
        let resp = response_to_json::<Resp>(&bytes)?;
        Ok(resp)
    }

    /// Send a wasmbus rpc message by wrapping with an Invocation before sending over nats.
    /// 'target' may be &str or String for sending to an actor, or a WasmCloudEntity (for actor or provider)
    /// If nats client is sync, this can block the current thread.
    /// If a response is not received within the default timeout, the Error RpcError::Timeout is returned.
    /// If the client timeout has been set, this call is equivalent to send_timeout passing in the
    /// default timeout.
    pub async fn send<Target>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        message: Message<'_>,
    ) -> RpcResult<Vec<u8>>
    where
        Target: Into<WasmCloudEntity>,
    {
        self.inner_rpc(origin, target, message, true, self.timeout).await
    }

    /// Send a wasmbus rpc message, with a timeout.
    /// The rpc message is wrapped with an Invocation before sending over nats.
    /// 'target' may be &str or String for sending to an actor, or a WasmCloudEntity (for actor or provider)
    /// If nats client is sync, this can block the current thread until either the response is received,
    /// or the timeout expires. If the timeout expires before the response is received,
    /// this returns Error RpcError::Timeout.
    pub async fn send_timeout<Target>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        message: Message<'_>,
        timeout: Duration,
    ) -> RpcResult<Vec<u8>>
    where
        Target: Into<WasmCloudEntity>,
    {
        self.inner_rpc(origin, target, message, true, Some(timeout)).await
    }

    /// Send a wasmbus rpc message without waiting for response.
    /// This has somewhat limited utility and is only useful if
    /// the message is declared to return no args, or if the caller
    /// doesn't care about the response.
    /// 'target' may be &str or String for sending to an actor,
    /// or a WasmCloudEntity (for actor or provider)
    #[doc(hidden)]
    pub async fn post<Target>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        message: Message<'_>,
    ) -> RpcResult<()>
    where
        Target: Into<WasmCloudEntity>,
    {
        let _ = self.inner_rpc(origin, target, message, false, None).await?;
        Ok(())
    }

    /// request or publish an rpc invocation
    #[instrument(level = "debug", skip(self, origin, target, message), fields(issuer = tracing::field::Empty, origin_url = tracing::field::Empty, subject = tracing::field::Empty, target_url = tracing::field::Empty, method = tracing::field::Empty))]
    async fn inner_rpc<Target>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        message: Message<'_>,
        expect_response: bool,
        timeout: Option<Duration>,
    ) -> RpcResult<Vec<u8>>
    where
        Target: Into<WasmCloudEntity>,
    {
        let target = target.into();
        let origin_url = origin.url();
        let subject = make_uuid();
        let issuer = &self.key.public_key();
        let raw_target_url = target.url();
        let target_url = format!("{}/{}", raw_target_url, &message.method);

        // Record all of the fields on the span. To avoid extra allocations, we are only going to
        // record here after we generate/derive the values
        let current_span = tracing::span::Span::current();
        current_span.record("issuer", &tracing::field::display(issuer));
        current_span.record("origin_url", &tracing::field::display(&origin_url));
        current_span.record("subject", &tracing::field::display(&subject));
        current_span.record("target_url", &tracing::field::display(&raw_target_url));
        current_span.record("method", &tracing::field::display(message.method));

        let claims = wascap::prelude::Claims::<wascap::prelude::Invocation>::new(
            issuer.clone(),
            subject.clone(),
            &target_url,
            &origin_url,
            &invocation_hash(&target_url, &origin_url, message.method, &message.arg),
        );

        let topic = rpc_topic(&target, &self.lattice_prefix);
        let method = message.method.to_string();
        let len = message.arg.len();
        let chunkify = {
            cfg_if::cfg_if! {
                if #[cfg(all(feature = "chunkify", not(target_arch = "wasm32")))] {
                    crate::chunkify::needs_chunking(len)
                } else {
                    false
                }
            }
        };

        #[allow(unused_variables)]
        let (invocation, body) = {
            let mut inv = Invocation {
                origin,
                target,
                operation: method.clone(),
                id: subject,
                encoded_claims: claims.encode(&self.key).unwrap(),
                host_id: self.host_id.clone(),
                content_length: Some(len as u64),
                ..Default::default()
            };
            if chunkify {
                (inv, Some(Vec::from(message.arg)))
            } else {
                inv.msg = Vec::from(message.arg);
                (inv, None)
            }
        };
        let nats_body = crate::common::serialize(&invocation)?;

        #[cfg(all(feature = "chunkify", not(target_arch = "wasm32")))]
        if let Some(body) = body {
            let inv_id = invocation.id.clone();
            debug!(invocation_id = %inv_id, %len, "chunkifying invocation");
            // start chunking thread
            let lattice = self.lattice_prefix().to_string();
            tokio::task::spawn_blocking(move || {
                let ce = chunkify_endpoint(None, lattice)?;
                ce.chunkify(&inv_id, &mut body.as_slice())
            })
            // I tried starting the send to ObjectStore in background thread,
            // and then send the rpc, but if the objectstore hasn't completed,
            // the recipient gets a missing object error,
            // so we need to flush this first
            .await
            // any errors sending chunks will cause send to fail with RpcError::Nats
            .map_err(|e| RpcError::Other(e.to_string()))??;
        }

        let timeout = if chunkify {
            timeout.map(|t| t + CHUNK_RPC_EXTRA_TIME)
        } else {
            timeout
        };
        trace!("rpc send");

        if expect_response {
            let payload = if let Some(timeout) = timeout {
                // if we expect a response before timeout, finish sending all chunks, then wait for response
                match tokio::time::timeout(timeout, self.request(&topic, &nats_body)).await {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(rpc_err)) => Err(RpcError::Nats(format!(
                        "rpc send error: {}: {}",
                        target_url, rpc_err
                    ))),
                    Err(timeout_err) => {
                        error!(error = %timeout_err, "rpc timeout: sending to target");
                        Err(RpcError::Timeout(format!(
                            "sending to {}: {}",
                            &target_url, timeout_err
                        )))
                    }
                }
            } else {
                // no timeout, wait indefinitely or until host times out
                self.request(&topic, &nats_body)
                    .await
                    .map_err(|e| RpcError::Nats(format!("rpc send error: {}: {}", target_url, e)))
            }?;

            let inv_response =
                crate::common::deserialize::<InvocationResponse>(&payload).map_err(|e| {
                    RpcError::Deser(format!("response to {}: {}", &method, &e.to_string()))
                })?;
            match inv_response.error {
                None => {
                    // was response chunked?
                    #[cfg(all(feature = "chunkify", not(target_arch = "wasm32")))]
                    let msg = if inv_response.content_length.is_some()
                        && inv_response.content_length.unwrap() > inv_response.msg.len() as u64
                    {
                        let lattice = self.lattice_prefix().to_string();
                        tokio::task::spawn_blocking(move || {
                            let ce = chunkify_endpoint(None, lattice)?;
                            ce.get_unchunkified_response(&inv_response.invocation_id)
                        })
                        .await
                        .map_err(|je| RpcError::Other(format!("join/resp-chunk: {}", je)))??
                    } else {
                        inv_response.msg
                    };
                    #[cfg(not(feature = "chunkify"))]
                    let msg = inv_response.msg;
                    trace!("rpc ok response");
                    Ok(msg)
                }
                Some(err) => {
                    // if error is Some(_), we must ignore the msg field
                    error!(error = %err, "rpc error response");
                    Err(RpcError::Rpc(err))
                }
            }
        } else {
            self.publish(&topic, &nats_body)
                .await
                .map_err(|e| RpcError::Nats(format!("rpc send error: {}: {}", target_url, e)))?;
            Ok(Vec::new())
        }
    }

    /// Send a nats message and wait for the response.
    /// This can be used for general nats messages, not just wasmbus actor/provider messages.
    /// If this client has a default timeout, and a response is not received within
    /// the appropriate time, an error will be returned.
    pub async fn request(&self, subject: &str, data: &[u8]) -> RpcResult<Vec<u8>> {
        use std::borrow::Borrow as _;

        let bytes = match self.client.borrow() {
            NatsClientType::Async(ref nats) => {
                #[cfg(feature = "otel")]
                let headers: Option<HeaderMap> =
                    Some(OtelHeaderInjector::default_with_span().into());
                #[cfg(not(feature = "otel"))]
                let headers: Option<HeaderMap> = None;
                let resp = request_with_headers_or_timeout(
                    nats,
                    subject,
                    headers.as_ref(),
                    self.timeout,
                    data,
                )
                .await
                .map_err(|e| RpcError::Nats(e.to_string()))?;
                resp.data
            }
            // These two never get invoked
            #[cfg(feature = "async_rewrite")]
            NatsClientType::AsyncRewrite(_) => unimplemented!(),
            //NatsClientType::Sync(_) => unimplemented!(),
        };
        Ok(bytes)
    }

    /// Send a nats message with no reply-to. Do not wait for a response.
    /// This can be used for general nats messages, not just wasmbus actor/provider messages.
    pub async fn publish(&self, subject: &str, data: &[u8]) -> RpcResult<()> {
        use std::borrow::Borrow as _;

        match self.client.borrow() {
            NatsClientType::Async(nats) => {
                #[cfg(feature = "otel")]
                let headers: Option<HeaderMap> =
                    Some(OtelHeaderInjector::default_with_span().into());
                #[cfg(not(feature = "otel"))]
                let headers: Option<HeaderMap> = None;
                nats.publish_with_reply_or_headers(subject, None, headers.as_ref(), data)
                    .await
                    .map_err(|e| RpcError::Nats(e.to_string()))?
            }
            // These two never get invoked
            #[cfg(feature = "async_rewrite")]
            NatsClientType::AsyncRewrite(_) => unimplemented!(),
        }
        Ok(())
    }
}

/// Copied straight from aflowt because the function isn't public
async fn request_with_headers_or_timeout(
    nats: &Connection,
    subject: &str,
    maybe_headers: Option<&HeaderMap>,
    maybe_timeout: Option<Duration>,
    msg: impl AsRef<[u8]>,
) -> std::io::Result<nats_aflowt::Message> {
    // Publish a request.
    let reply = nats.new_inbox();
    let sub = nats.subscribe(&reply).await?;
    nats.publish_with_reply_or_headers(subject, Some(reply.as_str()), maybe_headers, msg)
        .await?;

    // Wait for the response
    let result = if let Some(timeout) = maybe_timeout {
        sub.next_timeout(timeout).await
    } else if let Some(msg) = sub.next().await {
        Ok(msg)
    } else {
        Err(std::io::ErrorKind::ConnectionReset.into())
    };

    // Check for no responder status.
    if let Ok(msg) = result.as_ref() {
        if msg.is_no_responders() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no responders",
            ));
        }
    }

    result
}

pub(crate) fn invocation_hash(
    target_url: &str,
    origin_url: &str,
    method: &str,
    args: &[u8],
) -> String {
    let mut context = Context::new(&SHA256);
    context.update(origin_url.as_bytes());
    context.update(target_url.as_bytes());
    context.update(method.as_bytes());
    context.update(args);
    let digest = context.finish();
    data_encoding::HEXUPPER.encode(digest.as_ref())
}

/// Create a new random uuid for invocations.
/// Internally this (currently) uses the uuid crate, which uses 'getrandom',
/// which uses the operating system's random number generator.
/// See https://docs.rs/getrandom/0.2.3/getrandom/ for details
#[doc(hidden)]
pub fn make_uuid() -> String {
    use uuid::Uuid;
    // uuid uses getrandom, which uses the operating system's RNG
    // as the source of random numbers.
    Uuid::new_v4()
        .to_simple()
        .encode_lower(&mut Uuid::encode_buffer())
        .to_string()
}

/// A Json message (method, args)
struct JsonMessage<'m>(&'m str, JsonValue);

impl<'m> TryFrom<JsonMessage<'m>> for Message<'m> {
    type Error = RpcError;

    /// convert json message to rpc message (msgpack)
    fn try_from(jm: JsonMessage<'m>) -> Result<Message<'m>, Self::Error> {
        let arg = json_to_args::<JsonValue>(jm.1)?;
        Ok(Message {
            method: jm.0,
            arg: std::borrow::Cow::Owned(arg),
        })
    }
}

/// convert json args to msgpack
fn json_to_args<T>(v: JsonValue) -> RpcResult<Vec<u8>>
where
    T: Serialize,
    T: DeserializeOwned,
{
    crate::common::serialize(
        &serde_json::from_value::<T>(v)
            .map_err(|e| RpcError::Deser(format!("invalid params: {}.", e)))?,
    )
}

/// convert message response to json
fn response_to_json<T>(msg: &[u8]) -> RpcResult<JsonValue>
where
    T: Serialize,
    T: DeserializeOwned,
{
    serde_json::to_value(crate::common::deserialize::<T>(msg)?)
        .map_err(|e| RpcError::Ser(format!("response serialization : {}.", e)))
}
