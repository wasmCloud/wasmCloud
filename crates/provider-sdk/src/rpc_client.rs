use crate::{
    error::{InvocationError, InvocationResult, NetworkError, ValidationError},
    rpc_topic,
};

use std::{fmt, sync::Arc, time::Duration};

use async_nats::{Client, Subject};
use futures::TryFutureExt;
use sha2::Digest;
use tracing::{
    debug, error,
    field::{display, Empty},
    instrument,
};
use uuid::Uuid;
use wascap::{jwt, prelude::Claims};
use wasmcloud_core::{
    chunking::{ChunkEndpoint, CHUNK_RPC_EXTRA_TIME, CHUNK_THRESHOLD_BYTES},
    Invocation, InvocationResponse, WasmCloudEntity,
};
#[cfg(feature = "otel")]
use wasmcloud_tracing::context::TraceContextInjector;

/// Send wasmbus rpc messages
///
/// The primary use of RpcClient is providers sending to actors, however providers don't need to
/// construct this - it should be fetched from the provider connection
#[derive(Clone)]
pub struct RpcClient {
    client: Client,
    key: Arc<wascap::prelude::KeyPair>,
    /// host id (public key) for invocations
    host_id: String,
    /// timeout for rpc messages
    timeout: Option<Duration>,
    lattice: String,
    chonky: ChunkEndpoint,
}

// just so RpcClient can be included in other Debug structs
impl fmt::Debug for RpcClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcClient")
            .field("host_id", &self.host_id)
            .field("timeout", &self.timeout)
            .field("public_key", &self.key.public_key())
            .finish()
    }
}

impl RpcClient {
    /// Constructs a new RpcClient with an async nats connection.
    /// parameters: async nats client, host_id, optional timeout,
    /// secret key for signing messages,, and lattice id.
    pub fn new(
        nats: Client,
        host_id: String,
        timeout: Option<Duration>,
        key_pair: Arc<wascap::prelude::KeyPair>,
        lattice_id: &str,
    ) -> Self {
        // TODO(thomastaylor312): The original RPC code passes a None for the domain, but that seems
        // maybe wrong? We should probably be passing through a domain here but I don't want to
        // touch it without a second opinion as this code is some of our most tempermental.
        let chonky = ChunkEndpoint::with_client(lattice_id, nats.clone(), None::<&str>);
        RpcClient {
            client: nats,
            host_id,
            timeout,
            key: key_pair,
            lattice: lattice_id.to_string(),
            chonky,
        }
    }

    /// convenience method for returning the underlying NATS client
    pub fn client(&self) -> Client {
        self.client.clone()
    }

    pub async fn flush(&self) {
        if let Err(err) = self.client.flush().await {
            error!(%err, "error flushing NATS client");
        }
    }

    /// Send a wasmbus rpc message by wrapping with an Invocation before sending over nats.
    ///
    /// If a response is not received within the configured timeout, the error
    /// InvocationError::Timeout is returned.
    pub async fn send(
        &self,
        origin: WasmCloudEntity,
        target: WasmCloudEntity,
        method: impl Into<String>,
        data: Vec<u8>,
    ) -> InvocationResult<InvocationResponse> {
        self.inner_rpc(origin, target, method, data, self.timeout)
            .await
    }

    /// Send a wasmbus rpc message, with a timeout value that overrides the configured timeout for
    /// this client
    ///
    /// If the timeout expires before the response is received, this returns Error
    /// InvocationError::Timeout.
    pub async fn send_timeout(
        &self,
        origin: WasmCloudEntity,
        target: WasmCloudEntity,
        method: impl Into<String>,
        data: Vec<u8>,
        timeout: Duration,
    ) -> InvocationResult<InvocationResponse> {
        self.inner_rpc(origin, target, method, data, Some(timeout))
            .await
    }

    /// request or publish an rpc invocation
    #[instrument(level = "debug", skip(self, origin, target, method, data), fields( data_len = %data.len(), lattice_id = %self.lattice, method = Empty, subject = Empty, issuer = Empty, sender_key = Empty, contract_id = Empty, link_name = Empty, target_key = Empty, method = Empty, topic = Empty ))]
    async fn inner_rpc(
        &self,
        origin: WasmCloudEntity,
        target: WasmCloudEntity,
        method: impl Into<String>,
        data: Vec<u8>,
        timeout: Option<Duration>,
    ) -> InvocationResult<InvocationResponse> {
        let method = method.into();
        let origin_url = crate::url(&origin, None);
        let subject = make_uuid();
        let issuer = self.key.public_key();
        let target_url = crate::url(&target, Some(&method));
        let topic = rpc_topic(&target, &self.lattice);

        // Record all of the fields on the span. To avoid extra allocations, we are only going to
        // record here after we generate/derive the values
        let span = tracing::span::Span::current();
        span.record("method", display(&method));
        span.record("subject", &display(&subject));
        span.record("issuer", &display(&issuer));
        span.record("topic", &display(&topic));
        if !origin.public_key.is_empty() {
            span.record("sender_key", &display(&origin.public_key));
        }
        if !target.contract_id.is_empty() {
            span.record("contract_id", &display(&target.contract_id));
        }
        if !target.link_name.is_empty() {
            span.record("link_name", &display(&target.link_name));
        }
        if !target.public_key.is_empty() {
            span.record("target_key", &display(&target.public_key));
        }

        let claims = Claims::<jwt::Invocation>::new(
            issuer.clone(),
            subject.clone(),
            &target_url,
            &origin_url,
            &invocation_hash(&target_url, &origin_url, &method, &data),
        );

        let len = data.len();
        let needs_chunking = len > CHUNK_THRESHOLD_BYTES;

        let (invocation, body) = {
            let mut inv = Invocation {
                origin,
                target,
                operation: method.clone(),
                id: subject,
                encoded_claims: claims.encode(&self.key).unwrap_or_default(),
                host_id: self.host_id.clone(),
                content_length: len as u64,
                #[cfg(feature = "otel")]
                trace_context: TraceContextInjector::default_with_span().into(),
                ..Default::default()
            };
            if needs_chunking {
                (inv, Some(data))
            } else {
                inv.msg = data;
                (inv, None)
            }
        };
        let nats_body = crate::serialize(&invocation)?;
        if let Some(body) = body {
            debug!(invocation_id = %invocation.id, %len, "chunkifying invocation");

            if let Err(err) = self
                .chonky
                .chunkify(&invocation.id, &mut body.as_slice())
                .await
            {
                error!(%err, "chunking error");
                return Err(InvocationError::Chunking(err.to_string()));
            }
            // NOTE(thomastaylor312) This chunkify request is not sent as a separate thread because
            // we tried starting the send to ObjectStore in background thread, and then send the
            // rpc, but if the objectstore hasn't completed, the recipient gets a missing object
            // error
        }

        let timeout = if needs_chunking {
            timeout.map(|t| t + CHUNK_RPC_EXTRA_TIME)
        } else {
            timeout
        };

        let payload = self
            .request_timeout(topic, nats_body, timeout)
            .await
            .map_err(|err| {
                error!(%err, "sending request");
                err
            })?;

        let mut inv_response = crate::deserialize::<InvocationResponse>(&payload)?;
        if inv_response.error.is_none() {
            // was response chunked?
            let msg = if inv_response.content_length > inv_response.msg.len() as u64 {
                self.chonky
                    .get_unchunkified_response(&inv_response.invocation_id)
                    .await
                    .map_err(|e| InvocationError::Chunking(e.to_string()))?
            } else {
                inv_response.msg
            };
            inv_response.msg = msg;
        }

        Ok(inv_response)
    }

    /// Send a nats message and wait for the response.
    /// This can be used for general nats messages, not just wasmbus actor/provider messages.
    /// If this client has a default timeout, and a response is not received within
    /// the appropriate time, an error will be returned.
    #[instrument(level = "debug", skip_all, fields(subject = %subject))]
    pub async fn request(&self, subject: String, payload: Vec<u8>) -> InvocationResult<Vec<u8>> {
        self.request_timeout(subject, payload, self.timeout).await
    }

    async fn request_timeout(
        &self,
        subject: String,
        payload: Vec<u8>,
        timeout: Option<Duration>,
    ) -> InvocationResult<Vec<u8>> {
        let timeout = if timeout.is_none() {
            self.timeout
        } else {
            timeout
        };

        let req = async_nats::Request::new()
            .payload(payload.into())
            .timeout(timeout);
        match self.client.send_request(subject, req).await {
            Err(err) => {
                error!(%err, "error when performing NATS request");
                Err(match err.kind() {
                    async_nats::RequestErrorKind::TimedOut => InvocationError::Timeout,
                    _ => InvocationError::from(NetworkError::from(err)),
                })
            }
            Ok(message) => Ok(message.payload.into()),
        }
    }

    /// Send a nats message with no reply-to. Do not wait for a response.
    /// This can be used for general nats messages, not just wasmbus actor/provider messages.
    #[instrument(level = "trace", skip(self, payload))]
    pub(crate) async fn publish(&self, subject: Subject, payload: Vec<u8>) -> InvocationResult<()> {
        self.client
            .publish(subject, payload.into())
            .map_err(|e| InvocationError::from(NetworkError::from(e)))
            .await?;
        let nc = self.client();
        // TODO: revisit after doing some performance tuning and review of callers of pubish().
        // For high throughput use cases, it may be better to change the flush interval timer
        // instead of flushing after every publish.
        // Flushing here is good for low traffic use cases when optimizing for latency.
        tokio::spawn(async move {
            if let Err(err) = nc.flush().await {
                error!(%err, "flush after publish");
            }
        });
        Ok(())
    }

    pub(crate) async fn publish_invocation_response(
        &self,
        reply_to: Subject,
        response: InvocationResponse,
    ) -> InvocationResult<()> {
        let content_length = response.msg.len() as u64;
        let response = {
            if response.msg.len() > CHUNK_THRESHOLD_BYTES {
                self.chonky
                    .chunkify_response(&response.invocation_id, std::io::Cursor::new(response.msg))
                    .await
                    .map_err(|e| InvocationError::Chunking(e.to_string()))?;
                InvocationResponse {
                    msg: Vec::new(),
                    content_length,
                    ..response
                }
            } else {
                InvocationResponse {
                    content_length,
                    ..response
                }
            }
        };

        let data = crate::serialize(&response)?;
        self.publish(reply_to, data).await
    }

    pub async fn dechunk(&self, mut inv: Invocation) -> InvocationResult<Invocation> {
        if inv.content_length > inv.msg.len() as u64 {
            inv.msg = self
                .chonky
                .get_unchunkified(&inv.id)
                .await
                .map_err(|e| InvocationError::Chunking(e.to_string()))?;
        }
        Ok(inv)
    }

    /// Initial validation of received message. See provider::validate_provider_invocation for second part.
    pub async fn validate_invocation(
        &self,
        inv: Invocation,
    ) -> Result<(Invocation, Claims<jwt::Invocation>), ValidationError> {
        let vr = jwt::validate_token::<jwt::Invocation>(&inv.encoded_claims)
            .map_err(|e| ValidationError::InvalidJson(e.to_string()))?;
        if vr.expired {
            return Err(ValidationError::Expired);
        }
        if !vr.signature_valid {
            return Err(ValidationError::InvalidSignature);
        }
        if vr.cannot_use_yet {
            return Err(ValidationError::NotValidYet);
        }
        let target_url = crate::url(&inv.target, Some(&inv.operation));
        let hash = invocation_hash(
            &target_url,
            &crate::url(&inv.origin, None),
            &inv.operation,
            &inv.msg,
        );
        let claims = Claims::<jwt::Invocation>::decode(&inv.encoded_claims)
            .map_err(|e| ValidationError::InvalidJson(e.to_string()))?;
        let inv_claims = claims
            .metadata
            .as_ref()
            .ok_or(ValidationError::MissingWascapClaims)?;
        if inv_claims.invocation_hash != hash {
            return Err(ValidationError::HashMismatch);
        }
        if !inv.host_id.starts_with('N') && inv.host_id.len() != 56 {
            return Err(ValidationError::InvalidHostId(inv.host_id));
        }

        if inv_claims.target_url != target_url {
            return Err(ValidationError::InvalidTarget(
                inv_claims.target_url.to_owned(),
                target_url,
            ));
        }
        let origin_url = crate::url(&inv.origin, None);
        if inv_claims.origin_url != origin_url {
            return Err(ValidationError::InvalidOriginUrl(
                inv_claims.origin_url.to_owned(),
                origin_url,
            ));
        }
        Ok((inv, claims))
    }
}

pub(crate) fn invocation_hash(
    target_url: &str,
    origin_url: &str,
    method: &str,
    args: &[u8],
) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(origin_url.as_bytes());
    hasher.update(target_url.as_bytes());
    hasher.update(method.as_bytes());
    hasher.update(args);
    let digest = hasher.finalize();
    data_encoding::HEXUPPER.encode(digest.as_slice())
}

/// Create a new random uuid for invocations.
pub(crate) fn make_uuid() -> String {
    // uuid uses getrandom, which uses the operating system's RNG
    // as the source of random numbers.
    Uuid::new_v4()
        .as_simple()
        .encode_lower(&mut Uuid::encode_buffer())
        .to_string()
}
