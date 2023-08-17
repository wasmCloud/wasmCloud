/// wasmCloud host configuration
pub mod config;

pub use config::Host as HostConfig;

mod event;

use crate::{fetch_actor, socket_pair};

use core::fmt;
use core::future::Future;
use core::num::NonZeroUsize;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;

use std::collections::{hash_map, BTreeMap, HashMap};
use std::env;
use std::env::consts::{ARCH, FAMILY, OS};
use std::io::Cursor;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_nats::jetstream::{context::Context as JetstreamContext, kv};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::{BufMut, Bytes, BytesMut};
use cloudevents::{EventBuilder, EventBuilderV10};
use futures::stream::{AbortHandle, Abortable};
use futures::{stream, try_join, FutureExt, Stream, StreamExt, TryStreamExt};
use nkeys::{KeyPair, KeyPairType};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::io::{stderr, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{interval_at, Instant};
use tokio::{process, spawn};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn};
use ulid::Ulid;
use uuid::Uuid;
use wascap::jwt;
use wasmcloud_control_interface::{
    ActorAuctionAck, ActorAuctionRequest, ActorDescription, HostInventory, LinkDefinition,
    LinkDefinitionList, ProviderAuctionRequest, ProviderDescription, RemoveLinkDefinitionRequest,
    ScaleActorCommand, StartActorCommand, StartProviderCommand, StopActorCommand, StopHostCommand,
    StopProviderCommand, UpdateActorCommand,
};
use wasmcloud_runtime::capability::{messaging, Bus, KeyValueReadWrite, Messaging};
use wasmcloud_runtime::{ActorInstancePool, Runtime};

const SUCCESS: &str = r#"{"accepted":true,"error":""}"#;
const CLAIMS_PREFIX: &str = "CLAIMS_";
const LINKDEF_PREFIX: &str = "LINKDEF_";

#[derive(Debug)]
struct Queue {
    auction: async_nats::Subscriber,
    commands: async_nats::Subscriber,
    pings: async_nats::Subscriber,
    inventory: async_nats::Subscriber,
    links: async_nats::Subscriber,
    queries: async_nats::Subscriber,
    registries: async_nats::Subscriber,
}

impl Stream for Queue {
    type Item = async_nats::Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut pending = false;
        match Pin::new(&mut self.commands).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.links).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.queries).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.registries).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.inventory).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.auction).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.pings).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(None)
        }
    }
}

#[derive(Clone, Default)]
struct AsyncBytesMut(Arc<std::sync::Mutex<BytesMut>>);

impl AsyncWrite for AsyncBytesMut {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Poll::Ready({
            self.0
                .lock()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
                .put_slice(buf);
            Ok(buf.len())
        })
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl TryFrom<AsyncBytesMut> for Vec<u8> {
    type Error = anyhow::Error;

    fn try_from(buf: AsyncBytesMut) -> Result<Self, Self::Error> {
        buf.0
            .lock()
            .map(|buf| buf.clone().into())
            .map_err(|e| anyhow!(e.to_string()).context("failed to lock"))
    }
}

impl Queue {
    #[instrument]
    async fn new(
        nats: &async_nats::Client,
        lattice_prefix: &str,
        cluster_key: &KeyPair,
        host_key: &KeyPair,
    ) -> anyhow::Result<Self> {
        let host_id = host_key.public_key();
        let (registries, pings, links, queries, auction, commands, inventory) = try_join!(
            nats.subscribe(format!("wasmbus.ctl.{lattice_prefix}.registries.put",)),
            nats.subscribe(format!("wasmbus.ctl.{lattice_prefix}.ping.hosts",)),
            nats.subscribe(format!("wasmbus.ctl.{lattice_prefix}.linkdefs.*",)),
            nats.subscribe(format!("wasmbus.ctl.{lattice_prefix}.get.*",)),
            nats.subscribe(format!("wasmbus.ctl.{lattice_prefix}.auction.>",)),
            nats.subscribe(format!("wasmbus.ctl.{lattice_prefix}.cmd.{host_id}.*",)),
            nats.subscribe(format!("wasmbus.ctl.{lattice_prefix}.get.{host_id}.inv",)),
        )
        .context("failed to subscribe to queues")?;
        Ok(Self {
            auction,
            commands,
            pings,
            inventory,
            links,
            queries,
            registries,
        })
    }
}

#[derive(Debug)]
struct ActorInstance {
    nats: async_nats::Client,
    pool: ActorInstancePool,
    id: Ulid,
    calls: AbortHandle,
    runtime: Runtime,
    handler: Handler,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct WasmCloudEntity {
    link_name: String,
    contract_id: String,
    public_key: String,
}

impl fmt::Display for WasmCloudEntity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let url = self.url();
        write!(f, "{url}")
    }
}

impl WasmCloudEntity {
    /// The URL of the entity
    pub fn url(&self) -> String {
        if self.public_key.to_uppercase().starts_with('M') {
            format!("wasmbus://{}", self.public_key)
        } else {
            format!(
                "wasmbus://{}/{}/{}",
                self.contract_id
                    .replace(':', "/")
                    .replace(' ', "_")
                    .to_lowercase(),
                self.link_name.replace(' ', "_").to_lowercase(),
                self.public_key
            )
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Invocation {
    origin: WasmCloudEntity,
    target: WasmCloudEntity,
    operation: String,
    #[serde(with = "serde_bytes")]
    msg: Vec<u8>,
    id: String,
    encoded_claims: String,
    host_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_length: Option<u64>,
}

fn invocation_hash(
    target_url: impl AsRef<str>,
    origin_url: impl AsRef<str>,
    op: impl AsRef<str>,
    msg: impl AsRef<[u8]>,
) -> String {
    let mut hash = Sha256::default();
    hash.update(origin_url.as_ref());
    hash.update(target_url.as_ref());
    hash.update(op.as_ref());
    hash.update(msg.as_ref());
    hex::encode_upper(hash.finalize())
}

impl Invocation {
    /// Creates a new invocation. All invocations are signed with the host key as a way
    /// of preventing them from being forged over the network when connected to a lattice,
    /// so an invocation requires a reference to the host (signing) key
    pub fn new(
        hostkey: &KeyPair,
        origin: WasmCloudEntity,
        target: WasmCloudEntity,
        operation: String,
        msg: Vec<u8>,
    ) -> anyhow::Result<Invocation> {
        let id = Uuid::from_u128(Ulid::new().into()).to_string();
        let host_id = hostkey.public_key();
        let target_url = format!("{}/{operation}", target.url());
        let claims = jwt::Claims::<jwt::Invocation>::new(
            host_id.to_string(),
            id.to_string(),
            &target_url,
            &origin.url(),
            &invocation_hash(&target_url, origin.url(), &operation, &msg),
        );
        let encoded_claims = claims.encode(hostkey).context("failed to encode claims")?;

        Ok(Invocation {
            content_length: Some(msg.len() as _),
            origin,
            target,
            operation,
            msg,
            id,
            encoded_claims,
            host_id,
        })
    }
}

#[derive(Default, Deserialize, Serialize)]
struct InvocationResponse {
    #[serde(with = "serde_bytes")]
    msg: Vec<u8>,
    invocation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_length: Option<u64>,
}

#[derive(Clone, Debug)]
struct Handler {
    nats: async_nats::Client,
    lattice_prefix: String,
    cluster_key: Arc<KeyPair>,
    origin: WasmCloudEntity,
    interfaces: Arc<RwLock<HashMap<String, WasmCloudEntity>>>,
}

impl Handler {
    async fn invocation(
        &self,
        operation: impl AsRef<str>,
        request: Vec<u8>,
    ) -> anyhow::Result<Invocation> {
        let (package, interface_method) = operation
            .as_ref()
            .split_once('/')
            .context("failed to parse operation")?;
        let interfaces = self.interfaces.read().await;
        let target = interfaces.get(package).context("link not found")?;
        // TODO: Support per-interface links
        Invocation::new(
            &self.cluster_key,
            self.origin.clone(),
            target.clone(),
            interface_method.into(),
            request,
        )
    }

    #[instrument(skip(operation, request))]
    async fn call_provider_with_payload(
        &self,
        operation: impl AsRef<str>,
        request: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, String>> {
        let operation = operation.as_ref();
        let invocation = self.invocation(operation, request).await?;
        let request =
            rmp_serde::to_vec_named(&invocation).context("failed to encode invocation")?;
        let lattice_prefix = &self.lattice_prefix;
        let provider_id = invocation.target.public_key;
        let link_name = invocation.target.link_name;
        let res = self
            .nats
            .request(
                format!("wasmbus.rpc.{lattice_prefix}.{provider_id}.{link_name}"),
                request.into(),
            )
            .await
            .context("failed to publish on NATS topic")?;
        let InvocationResponse {
            invocation_id,
            msg,
            content_length,
            error,
        } = rmp_serde::from_slice(&res.payload).context("failed to decode invocation response")?;
        ensure!(invocation_id == invocation.id, "invocation ID mismatch");
        if let Some(content_length) = content_length {
            let content_length =
                usize::try_from(content_length).context("content length does not fit in usize")?;
            ensure!(content_length == msg.len(), "message size mismatch");
        }
        if let Some(error) = error {
            Ok(Err(error))
        } else {
            Ok(Ok(msg))
        }
    }

    #[instrument(skip(operation, request))]
    async fn call_provider(
        &self,
        operation: impl AsRef<str>,
        request: &impl Serialize,
    ) -> anyhow::Result<Vec<u8>> {
        let operation = operation.as_ref();
        let request = rmp_serde::to_vec_named(request).context("failed to encode request")?;
        self.call_provider_with_payload(operation, request)
            .await
            .context("failed to call linked provider")?
            .map_err(|err| anyhow!(err).context(format!("`{operation}` call failed")))
    }
}

#[async_trait]
impl Bus for Handler {
    #[instrument]
    async fn call(
        &self,
        operation: String,
    ) -> anyhow::Result<(
        Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
        Box<dyn AsyncWrite + Sync + Send + Unpin>,
        Box<dyn AsyncRead + Sync + Send + Unpin>,
    )> {
        let (package, interface_method) = operation
            .split_once('/')
            .context("failed to parse operation")?;
        let interfaces = self.interfaces.read().await;
        let target = interfaces.get(package).context("link not found")?;
        // TODO: Support per-interface links
        let (mut req_r, req_w) = socket_pair()?;
        let (res_r, mut res_w) = socket_pair()?;

        let nats = self.nats.clone();
        let lattice_prefix = self.lattice_prefix.clone();
        let origin = self.origin.clone();
        let target = target.clone();
        let cluster_key = self.cluster_key.clone();
        let interface_method = interface_method.to_string();
        Ok((
            async move {
                // TODO: Stream data
                let mut request = vec![];
                req_r
                    .read_to_end(&mut request)
                    .await
                    .context("failed to read request")
                    .map_err(|e| e.to_string())?;
                let invocation =
                    Invocation::new(&cluster_key, origin, target, interface_method, request)
                        .map_err(|e| e.to_string())?;
                let provider_id = &invocation.target.public_key;
                let link_name = &invocation.target.link_name;
                let request = rmp_serde::to_vec_named(&invocation)
                    .context("failed to encode invocation")
                    .map_err(|e| e.to_string())?;
                let res = nats
                    .request(
                        format!("wasmbus.rpc.{lattice_prefix}.{provider_id}.{link_name}"),
                        request.into(),
                    )
                    .await
                    .context("failed to call provider")
                    .map_err(|e| e.to_string())?;
                let InvocationResponse {
                    invocation_id,
                    msg,
                    content_length,
                    error,
                } = rmp_serde::from_slice(&res.payload)
                    .context("failed to decode invocation response")
                    .map_err(|e| e.to_string())?;
                if invocation_id != invocation.id {
                    return Err("invocation ID mismatch".into());
                }
                if let Some(content_length) = content_length {
                    let content_length = usize::try_from(content_length)
                        .context("content length does not fit in usize")
                        .map_err(|e| e.to_string())?;
                    if content_length != msg.len() {
                        return Err("message size mismatch".into());
                    }
                }
                if let Some(error) = error {
                    Err(error)
                } else {
                    res_w
                        .write_all(&msg)
                        .await
                        .context("failed to write reply")
                        .map_err(|e| e.to_string())?;
                    Ok(())
                }
            }
            .boxed(),
            Box::new(req_w),
            Box::new(res_r),
        ))
    }

    async fn call_oneshot(
        &self,
        operation: String,
        request: Vec<u8>,
    ) -> anyhow::Result<Result<(), String>> {
        match self
            .call_provider_with_payload(&operation, request)
            .await
            .context("failed to call linked provider")?
        {
            Ok(msg) => {
                if !msg.is_empty() {
                    error!("unexpected response returned for `{operation}` call");
                }
                Ok(Ok(()))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    async fn call_oneshot_with_response(
        &self,
        operation: String,
        request: Vec<u8>,
        response: &mut Vec<u8>,
    ) -> anyhow::Result<Result<usize, String>> {
        match self
            .call_provider_with_payload(operation, request)
            .await
            .context("failed to call linked provider")?
        {
            Ok(msg) => {
                let size = msg.len();
                response.resize(size, 0);
                response.copy_from_slice(&msg);
                Ok(Ok(size))
            }
            Err(err) => Ok(Err(err)),
        }
    }
}

#[async_trait]
impl KeyValueReadWrite for Handler {
    #[instrument]
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)> {
        #[derive(Deserialize)]
        struct GetResponse {
            #[serde(default)]
            value: String,
            #[serde(default)]
            exists: bool,
        }

        const METHOD: &str = "wasmcloud:keyvalue/KeyValue.Get";
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let res = self.call_provider(METHOD, &key).await?;
        let GetResponse { value, exists } =
            rmp_serde::from_slice(&res).context("failed to decode response")?;
        if !exists {
            bail!("key not found")
        }
        let size = value
            .len()
            .try_into()
            .context("value size does not fit in `u64`")?;
        Ok((Box::new(Cursor::new(value)), size))
    }

    #[instrument(skip(value))]
    async fn set(
        &self,
        bucket: &str,
        key: String,
        mut value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct SetRequest {
            key: String,
            value: String,
            expires: u32,
        }

        const METHOD: &str = "wasmcloud:keyvalue/KeyValue.Set";
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let mut buf = String::new();
        value
            .read_to_string(&mut buf)
            .await
            .context("failed to read value")?;
        let res = self
            .call_provider(
                METHOD,
                &SetRequest {
                    key,
                    value: buf,
                    expires: 0,
                },
            )
            .await?;
        if !res.is_empty() {
            error!("unexpected response returned for `{METHOD}` call");
        }
        Ok(())
    }

    #[instrument]
    async fn delete(&self, bucket: &str, key: String) -> anyhow::Result<()> {
        const METHOD: &str = "wasmcloud:keyvalue/KeyValue.Del";
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let res = self.call_provider(METHOD, &key).await?;
        let deleted: bool = rmp_serde::from_slice(&res).context("failed to decode response")?;
        ensure!(deleted, "key not found");
        Ok(())
    }

    #[instrument]
    async fn exists(&self, bucket: &str, key: String) -> anyhow::Result<bool> {
        const METHOD: &str = "wasmcloud:keyvalue/KeyValue.Contains";
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let res = self.call_provider(METHOD, &key).await?;
        rmp_serde::from_slice(&res).context("failed to decode response")
    }
}

#[async_trait]
impl Messaging for Handler {
    #[instrument]
    async fn request(
        &self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> anyhow::Result<messaging::types::BrokerMessage> {
        #[derive(Serialize)]
        struct RequestMessage {
            subject: String,
            #[serde(with = "serde_bytes")]
            body: Vec<u8>,
            #[serde(rename = "timeoutMs")]
            timeout_ms: u32,
        }
        #[derive(Deserialize)]
        struct ReplyMessage {
            #[serde(default)]
            subject: String,
            #[serde(rename = "replyTo")]
            #[serde(default, skip_serializing_if = "Option::is_none")]
            reply_to: Option<String>,
            #[serde(with = "serde_bytes")]
            #[serde(default)]
            body: Vec<u8>,
        }
        const METHOD: &str = "wasmcloud:messaging/Messaging.Request";

        let timeout_ms = timeout
            .as_millis()
            .try_into()
            .context("timeout milliseconds do not fit in `u32`")?;
        let res = self
            .call_provider(
                METHOD,
                &RequestMessage {
                    subject,
                    body: body.unwrap_or_default(),
                    timeout_ms,
                },
            )
            .await?;
        let ReplyMessage {
            subject,
            reply_to,
            body,
        } = rmp_serde::from_slice(&res).context("failed to decode response")?;
        Ok(messaging::types::BrokerMessage {
            subject,
            reply_to,
            body: Some(body),
        })
    }

    #[instrument]
    async fn request_multi(
        &self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout: Duration,
        max_results: u32,
    ) -> anyhow::Result<Vec<messaging::types::BrokerMessage>> {
        match max_results {
            0..=1 => {
                let res = self.request(subject, body, timeout).await?;
                Ok(vec![res])
            }
            2.. => bail!("at most 1 result can be requested at the time"),
        }
    }

    #[instrument]
    async fn publish(
        &self,
        messaging::types::BrokerMessage {
            subject,
            reply_to,
            body,
        }: messaging::types::BrokerMessage,
    ) -> anyhow::Result<()> {
        #[derive(Serialize)]
        struct PubMessage {
            subject: String,
            #[serde(rename = "replyTo")]
            #[serde(skip_serializing_if = "Option::is_none")]
            reply_to: Option<String>,
            #[serde(with = "serde_bytes")]
            body: Vec<u8>,
        }

        const METHOD: &str = "wasmcloud:messaging/Messaging.Publish";
        let res = self
            .call_provider(
                METHOD,
                &PubMessage {
                    subject,
                    reply_to,
                    body: body.unwrap_or_default(),
                },
            )
            .await?;
        if !res.is_empty() {
            error!("unexpected response returned for `{METHOD}` call");
        }
        Ok(())
    }
}

impl ActorInstance {
    #[instrument(skip(self, payload))]
    async fn handle_call(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        let Invocation {
            origin,
            target,
            operation,
            msg,
            id: invocation_id,
            ..
        } = rmp_serde::from_slice(payload.as_ref()).context("failed to decode invocation")?;

        debug!(?origin, ?target, operation, "handle actor invocation");

        let mut instance = self
            .pool
            .instantiate(self.runtime.clone())
            .await
            .context("failed to instantiate actor")?;
        instance
            .stderr(stderr())
            .await
            .context("failed to set stderr")?
            .bus(Arc::new(self.handler.clone()))
            .keyvalue_readwrite(Arc::new(self.handler.clone()))
            .messaging(Arc::new(self.handler.clone()));
        let res = AsyncBytesMut::default();
        let res = match instance
            .call(operation, Cursor::new(msg), res.clone())
            .await
            .context("failed to call actor")?
        {
            Ok(()) => {
                let msg: Vec<_> = res.try_into()?;
                let content_length = msg.len().try_into().ok();
                InvocationResponse {
                    msg,
                    invocation_id,
                    content_length,
                    ..Default::default()
                }
            }
            Err(e) => InvocationResponse {
                invocation_id,
                error: Some(e),
                ..Default::default()
            },
        };
        rmp_serde::to_vec_named(&res)
            .map(Into::into)
            .context("failed to encode response")
    }

    #[instrument(skip(self))]
    async fn handle_message(
        &self,
        async_nats::Message {
            reply,
            payload,
            subject,
            ..
        }: async_nats::Message,
    ) {
        let res = self.handle_call(payload).await;
        match (reply, res) {
            (Some(reply), Ok(buf)) => {
                if let Err(e) = self.nats.publish(reply, buf).await {
                    error!("failed to publish response to `{subject}` request: {e:?}");
                }
            }
            (_, Err(e)) => {
                warn!("failed to handle `{subject}` request: {e:?}");
            }
            _ => {}
        }
    }
}

type Annotations = BTreeMap<String, String>;

#[derive(Debug)]
struct Actor {
    pool: ActorInstancePool,
    instances: RwLock<HashMap<Option<Annotations>, Vec<Arc<ActorInstance>>>>,
    image_ref: String,
    handler: Handler,
}

#[derive(Debug)]
struct ProviderInstance {
    child: JoinHandle<()>,
    id: Ulid,
    annotations: Option<Annotations>,
}

#[derive(Debug)]
struct Provider {
    claims: jwt::Claims<jwt::CapabilityProvider>,
    instances: HashMap<String, ProviderInstance>,
    image_ref: String,
}

/// wasmCloud Host
#[derive(Debug)]
pub struct Host {
    // TODO: Clean up actors after stop
    actors: RwLock<HashMap<String, Arc<Actor>>>,
    cluster_key: Arc<KeyPair>,
    event_builder: EventBuilderV10,
    friendly_name: String,
    heartbeat: AbortHandle,
    host_config: HostConfig,
    host_key: KeyPair,
    labels: HashMap<String, String>,
    nats: async_nats::Client,
    data: kv::Store,
    data_watch: AbortHandle,
    providers: RwLock<HashMap<String, Provider>>,
    runtime: Runtime,
    start_at: Instant,
    stop_tx: watch::Sender<Option<Instant>>,
    stop_rx: watch::Receiver<Option<Instant>>,
    queue: AbortHandle,
    links: RwLock<HashMap<String, LinkDefinition>>,
}

fn linkdef_hash(
    actor_id: impl AsRef<str>,
    contract_id: impl AsRef<str>,
    link_name: impl AsRef<str>,
) -> String {
    let mut hash = Sha256::default();
    hash.update(actor_id.as_ref());
    hash.update(contract_id.as_ref());
    hash.update(link_name.as_ref());
    hex::encode_upper(hash.finalize())
}

#[instrument(skip(jetstream))]
async fn create_lattice_metadata_bucket(
    jetstream: &JetstreamContext,
    bucket: &str,
) -> anyhow::Result<()> {
    // Don't create the bucket if it already exists
    if let Ok(_store) = jetstream.get_key_value(bucket).await {
        info!("lattice metadata bucket {bucket} already exists. Skipping creation.");
        return Ok(());
    }

    match jetstream
        .create_key_value(kv::Config {
            bucket: bucket.to_string(),
            ..Default::default()
        })
        .await
    {
        Ok(_) => {
            info!("created lattice metadata bucket {bucket} with 1 replica");
            Ok(())
        }
        Err(err) => Err(anyhow!(err).context(format!(
            "failed to create lattice metadata bucket '{bucket}'"
        ))),
    }
}

impl Host {
    const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

    const NAME_ADJECTIVES: &str = "
    autumn hidden bitter misty silent empty dry dark summer
    icy delicate quiet white cool spring winter patient
    twilight dawn crimson wispy weathered blue billowing
    broken cold damp falling frosty green long late lingering
    bold little morning muddy old red rough still small
    sparkling bouncing shy wandering withered wild black
    young holy solitary fragrant aged snowy proud floral
    restless divine polished ancient purple lively nameless
    gray orange mauve
    ";

    const NAME_NOUNS: &str = "
    waterfall river breeze moon rain wind sea morning
    snow lake sunset pine shadow leaf dawn glitter forest
    hill cloud meadow sun glade bird brook butterfly
    bush dew dust field fire flower firefly ladybug feather grass
    haze mountain night pond darkness snowflake silence
    sound sky shape stapler surf thunder violet water wildflower
    wave water resonance sun wood dream cherry tree fog autocorrect
    frost voice paper frog smoke star hamster ocean emoji robot
    ";

    /// Generate a friendly name for the host based on a random number.
    /// Names we pulled from a list of friendly or neutral adjectives and nouns suitable for use in
    /// public and on hosts/domain names
    fn generate_friendly_name() -> Option<String> {
        let adjectives: Vec<_> = Self::NAME_ADJECTIVES.split_whitespace().collect();
        let nouns: Vec<_> = Self::NAME_NOUNS.split_whitespace().collect();
        names::Generator::new(&adjectives, &nouns, names::Name::Numbered).next()
    }

    /// Construct a new [Host] returning a tuple of its [Arc] and an async shutdown function.
    #[instrument]
    pub async fn new(
        config: HostConfig,
    ) -> anyhow::Result<(Arc<Self>, impl Future<Output = anyhow::Result<()>>)> {
        let cluster_key = if let Some(cluster_seed) = config.cluster_seed.as_ref() {
            let kp = KeyPair::from_seed(cluster_seed)
                .context("failed to construct key pair from seed")?;
            ensure!(kp.key_pair_type() == KeyPairType::Cluster);
            kp
        } else {
            KeyPair::new(KeyPairType::Cluster)
        };
        if let Some(issuers) = config.cluster_issuers.as_ref() {
            if !issuers.contains(&cluster_key.public_key()) {
                bail!("cluster issuers list must contain the cluster key");
            }
        }
        let cluster_key = Arc::new(cluster_key);
        let host_key = if let Some(host_seed) = config.host_seed.as_ref() {
            let kp =
                KeyPair::from_seed(host_seed).context("failed to construct key pair from seed")?;
            ensure!(kp.key_pair_type() == KeyPairType::Server);
            kp
        } else {
            KeyPair::new(KeyPairType::Server)
        };

        let mut labels = HashMap::from([
            ("hostcore.arch".into(), ARCH.into()),
            ("hostcore.os".into(), OS.into()),
            ("hostcore.osfamily".into(), FAMILY.into()),
        ]);
        labels.extend(env::vars().filter_map(|(k, v)| {
            let k = k.strip_prefix("HOST_")?;
            Some((k.to_lowercase(), v))
        }));
        let friendly_name =
            Self::generate_friendly_name().context("failed to generate friendly name")?;

        let start_evt = json!({
            "friendly_name": friendly_name,
            "labels": labels,
            "uptime_seconds": 0,
            "version": env!("CARGO_PKG_VERSION"),
        });

        debug!(
            ctl_nats_url = config.ctl_nats_url.as_str(),
            "connecting to NATS control server"
        );
        let nats = async_nats::connect(config.ctl_nats_url.as_str())
            .await
            .context("failed to connect to NATS control server")?;

        let queue = Queue::new(&nats, &config.lattice_prefix, &cluster_key, &host_key)
            .await
            .context("failed to initialize queue")?;
        nats.flush().await.context("failed to flush")?;

        let start_at = Instant::now();

        let heartbeat_start_at = start_at
            .checked_add(Self::HEARTBEAT_INTERVAL)
            .context("failed to compute heartbeat start time")?;
        let heartbeat =
            IntervalStream::new(interval_at(heartbeat_start_at, Self::HEARTBEAT_INTERVAL));

        let (stop_tx, stop_rx) = watch::channel(None);

        // TODO: Configure
        let runtime = Runtime::builder()
            .actor_config(wasmcloud_runtime::ActorConfig {
                require_signature: true,
            })
            .build()
            .context("failed to build runtime")?;
        let event_builder = EventBuilderV10::new().source(host_key.public_key());

        let jetstream = if let Some(domain) = config.js_domain.as_ref() {
            async_nats::jetstream::with_domain(nats.clone(), domain)
        } else {
            async_nats::jetstream::new(nats.clone())
        };
        let bucket = format!("LATTICEDATA_{}", config.lattice_prefix);
        create_lattice_metadata_bucket(&jetstream, &bucket).await?;

        let data = jetstream
            .get_key_value(&bucket)
            .await
            .map_err(|e| anyhow!(e).context("failed to acquire data bucket"))?;

        let (queue_abort, queue_abort_reg) = AbortHandle::new_pair();
        let (heartbeat_abort, heartbeat_abort_reg) = AbortHandle::new_pair();
        let (data_watch_abort, data_watch_abort_reg) = AbortHandle::new_pair();

        let host = Host {
            actors: RwLock::default(),
            cluster_key,
            event_builder,
            friendly_name,
            heartbeat: heartbeat_abort.clone(),
            host_config: config,
            host_key,
            labels,
            nats,
            data: data.clone(),
            data_watch: data_watch_abort.clone(),
            providers: RwLock::default(),
            runtime,
            start_at,
            stop_rx,
            stop_tx,
            queue: queue_abort.clone(),
            links: RwLock::default(),
        };
        host.publish_event("host_started", start_evt)
            .await
            .context("failed to publish start event")?;
        info!("host {} started", host.host_key.public_key());

        let host = Arc::new(host);
        let queue = spawn({
            let host = Arc::clone(&host);
            async {
                Abortable::new(queue, queue_abort_reg)
                    .for_each(move |msg| {
                        let host = Arc::clone(&host);
                        async move { host.handle_message(msg).await }
                    })
                    .await;
            }
        });
        let data_watch: JoinHandle<anyhow::Result<_>> = spawn({
            let host = Arc::clone(&host);
            async move {
                let data_watch = data
                    .watch_with_history(">")
                    .await
                    .context("failed to watch lattice data bucket")?;
                Abortable::new(data_watch, data_watch_abort_reg)
                    .for_each(move |entry| {
                        let host = Arc::clone(&host);
                        async move {
                            match entry {
                                Err(error) => {
                                    error!("failed to watch lattice data bucket: {error}");
                                }
                                Ok(entry) => host.process_entry(entry).await,
                            }
                        }
                    })
                    .await;
                Ok(())
            }
        });
        let heartbeat = spawn({
            let host = Arc::clone(&host);
            Abortable::new(heartbeat, heartbeat_abort_reg).for_each(move |_| {
                let host = Arc::clone(&host);
                async move {
                    let heartbeat = host.heartbeat().await;
                    if let Err(e) = host.publish_event("host_heartbeat", heartbeat).await {
                        error!("failed to publish heartbeat: {e}");
                    }
                }
            })
        });
        Ok((Arc::clone(&host), async move {
            heartbeat_abort.abort();
            queue_abort.abort();
            data_watch_abort.abort();
            let _ = try_join!(queue, data_watch, heartbeat).context("failed to await tasks")?;
            host.publish_event(
                "host_stopped",
                json!({
                    "labels": host.labels,
                }),
            )
            .await
            .context("failed to publish stop event")
        }))
    }

    /// Waits for host to be stopped via lattice commands and returns the shutdown deadline on
    /// success
    ///
    /// # Errors
    ///
    /// Returns an error if internal stop channel is closed prematurely
    #[instrument(skip(self))]
    pub async fn stopped(&self) -> anyhow::Result<Option<Instant>> {
        self.stop_rx
            .clone()
            .changed()
            .await
            .context("failed to wait for stop")?;
        Ok(*self.stop_rx.borrow())
    }

    #[instrument(skip(self))]
    async fn heartbeat(&self) -> serde_json::Value {
        let actors = self.actors.read().await;
        let actors: HashMap<&String, usize> = stream::iter(actors.iter())
            .filter_map(|(id, actor)| async move {
                let instances = actor.instances.read().await;
                let count = instances.values().map(Vec::len).sum();
                if count == 0 {
                    None
                } else {
                    Some((id, count))
                }
            })
            .collect()
            .await;
        let providers: Vec<_> = self
            .providers
            .read()
            .await
            .iter()
            .flat_map(
                |(
                    public_key,
                    Provider {
                        claims, instances, ..
                    },
                )| {
                    instances.keys().map(move |link_name| {
                        let metadata = claims.metadata.as_ref();
                        let contract_id =
                            metadata.map(|jwt::CapabilityProvider { capid, .. }| capid.as_str());
                        json!({
                            "public_key": public_key,
                            "link_name": link_name,
                            "contract_id": contract_id.unwrap_or("n/a"),
                        })
                    })
                },
            )
            .collect();
        let uptime = self.start_at.elapsed();
        json!({
            "actors": actors,
            "friendly_name": self.friendly_name,
            "labels": self.labels,
            "providers": providers,
            "uptime_human": "TODO", // TODO
            "uptime_seconds": uptime.as_secs(),
            "version": env!("CARGO_PKG_VERSION"),
        })
    }

    #[instrument(skip(self, name))]
    async fn publish_event(
        &self,
        name: impl AsRef<str>,
        data: serde_json::Value,
    ) -> anyhow::Result<()> {
        let name = name.as_ref();
        let name = format!("com.wasmcloud.lattice.{name}");
        let now = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .context("failed to format current time")?;
        let ev = self
            .event_builder
            .clone()
            .ty(&name)
            .id(Uuid::from_u128(Ulid::new().into()).to_string())
            .time(now)
            .data("application/json", data)
            .build()
            .context("failed to build cloud event")?;
        let ev = serde_json::to_vec(&ev).context("failed to serialize event")?;
        self.nats
            .publish(
                format!("wasmbus.evt.{}", self.host_config.lattice_prefix),
                ev.into(),
            )
            .await
            .with_context(|| format!("failed to publish `{name}` event"))
    }

    /// Instantiate an actor and publish the actor start events.
    #[allow(clippy::too_many_arguments)] // TODO: refactor into a config struct
    #[instrument(skip(self, host_id, actor_ref))]
    async fn instantiate_actor(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        annotations: &Option<Annotations>,
        host_id: impl AsRef<str>,
        actor_ref: impl AsRef<str>,
        count: NonZeroUsize,
        pool: ActorInstancePool,
        handler: Handler,
    ) -> anyhow::Result<Vec<Arc<ActorInstance>>> {
        trace!(actor_ref = actor_ref.as_ref(), count, "instantiating actor");

        let actor_ref = actor_ref.as_ref();
        let instances = stream::repeat(format!(
            "wasmbus.rpc.{lattice_prefix}.{subject}",
            lattice_prefix = self.host_config.lattice_prefix,
            subject = claims.subject
        ))
        .take(count.into())
        .then(|topic| {
            let pool = pool.clone();
            let handler = handler.clone();
            async move {
                let calls = self
                    .nats
                    .queue_subscribe(topic.clone(), topic)
                    .await
                    .context("failed to subscribe to actor call queue")?;

                let (calls_abort, calls_abort_reg) = AbortHandle::new_pair();
                let id = Ulid::new();
                let instance = Arc::new(ActorInstance {
                    nats: self.nats.clone(),
                    pool,
                    id,
                    calls: calls_abort,
                    runtime: self.runtime.clone(),
                    handler: handler.clone(),
                });

                let _calls = spawn({
                    let instance = Arc::clone(&instance);
                    Abortable::new(calls, calls_abort_reg).for_each_concurrent(None, move |msg| {
                        let instance = Arc::clone(&instance);
                        async move { instance.handle_message(msg).await }
                    })
                });

                self.publish_event(
                    "actor_started",
                    event::actor_started(
                        claims,
                        annotations,
                        Uuid::from_u128(id.into()),
                        actor_ref,
                    ),
                )
                .await?;
                anyhow::Result::<_>::Ok(instance)
            }
        })
        .try_collect()
        .await
        .context("failed to instantiate actor")?;
        self.publish_event(
            "actors_started",
            event::actors_started(claims, annotations, host_id, count, actor_ref),
        )
        .await?;
        Ok(instances)
    }

    /// Uninstantiate an actor and publish the actor stop events.
    #[instrument(skip(self))]
    async fn uninstantiate_actor(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        annotations: &Option<Annotations>,
        host_id: &str,
        instances: &mut Vec<Arc<ActorInstance>>,
        count: NonZeroUsize,
        remaining: usize,
    ) -> anyhow::Result<()> {
        trace!(
            subject = claims.subject,
            count,
            remaining,
            "uninstantiating actor instances"
        );

        stream::iter(instances.drain(..usize::from(count)))
            .map(Ok)
            .try_for_each_concurrent(None, |instance| {
                instance.calls.abort();
                async move {
                    self.publish_event(
                        "actor_stopped",
                        event::actor_stopped(
                            claims,
                            annotations,
                            Uuid::from_u128(instance.id.into()),
                        ),
                    )
                    .await
                }
            })
            .await?;
        self.publish_event(
            "actors_stopped",
            event::actors_stopped(claims, annotations, host_id, count, remaining),
        )
        .await
    }

    #[instrument(skip(self, annotations))]
    async fn start_actor<'a>(
        &self,
        entry: hash_map::VacantEntry<'a, String, Arc<Actor>>,
        actor: wasmcloud_runtime::Actor,
        actor_ref: String,
        count: NonZeroUsize,
        host_id: &str,
        annotations: Option<impl Into<Annotations>>,
    ) -> anyhow::Result<&'a mut Arc<Actor>> {
        trace!(actor_ref, "starting new actor");

        let annotations = annotations.map(Into::into);
        let claims = actor.claims().context("claims missing")?;
        let links = self.links.read().await;
        let interfaces = links
            .values()
            .filter_map(|ld| {
                (ld.actor_id == claims.subject).then(|| {
                    (
                        ld.contract_id.clone(),
                        WasmCloudEntity {
                            link_name: ld.link_name.clone(),
                            contract_id: ld.contract_id.clone(),
                            public_key: ld.provider_id.clone(),
                        },
                    )
                })
            })
            .collect();
        let origin = WasmCloudEntity {
            public_key: claims.subject.clone(),
            ..Default::default()
        };
        let handler = Handler {
            nats: self.nats.clone(),
            lattice_prefix: self.host_config.lattice_prefix.clone(),
            origin,
            cluster_key: Arc::clone(&self.cluster_key),
            interfaces: Arc::new(RwLock::new(interfaces)),
        };

        let pool = ActorInstancePool::new(actor.clone(), Some(count));
        let instances = self
            .instantiate_actor(
                claims,
                &annotations,
                host_id,
                &actor_ref,
                count,
                pool.clone(),
                handler.clone(),
            )
            .await
            .context("failed to instantiate actor")?;
        let actor = Arc::new(Actor {
            pool,
            instances: RwLock::new(HashMap::from([(annotations, instances)])),
            image_ref: actor_ref,
            handler,
        });
        Ok(entry.insert(actor))
    }

    #[instrument(skip(self))]
    async fn stop_actor<'a>(
        &self,
        entry: hash_map::OccupiedEntry<'a, String, Arc<Actor>>,
        host_id: &str,
    ) -> anyhow::Result<()> {
        let actor = entry.remove();
        let claims = actor.pool.claims().context("claims missing")?;
        let mut instances = actor.instances.write().await;
        let remaining: usize = instances.values().map(Vec::len).sum();
        stream::iter(instances.drain())
            .map(anyhow::Result::<_>::Ok)
            .try_fold(
                remaining,
                |remaining, (annotations, mut instances)| async move {
                    let Some(count) = NonZeroUsize::new(instances.len()) else {
                        return Ok(remaining)
                    };
                    let remaining = remaining
                        .checked_sub(count.into())
                        .context("invalid instance length")?;
                    self.uninstantiate_actor(
                        claims,
                        &annotations,
                        host_id,
                        &mut instances,
                        count,
                        remaining,
                    )
                    .await
                    .context("failed to uninstantiate actor")?;
                    Ok(remaining)
                },
            )
            .await?;
        Ok(())
    }

    #[instrument(skip(self, payload))]
    async fn handle_auction_actor(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        let ActorAuctionRequest {
            actor_ref,
            constraints,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor auction command")?;

        debug!(actor_ref, ?constraints, "auction actor");

        let buf = serde_json::to_vec(&ActorAuctionAck {
            actor_ref,
            constraints,
            host_id: self.host_key.public_key(),
        })
        .context("failed to encode reply")?;
        Ok(buf.into())
    }

    #[instrument(skip(self, payload))]
    async fn handle_auction_provider(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<Option<Bytes>> {
        let ProviderAuctionRequest {
            provider_ref,
            constraints,
            link_name,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider auction command")?;

        debug!(provider_ref, link_name, ?constraints, "auction provider");

        let providers = self.providers.read().await;
        if providers.values().any(
            |Provider {
                 image_ref,
                 instances,
                 ..
             }| { *image_ref == provider_ref && instances.contains_key(&link_name) },
        ) {
            // Do not reply if the provider is already running
            return Ok(None);
        }

        // TODO: ProviderAuctionAck is missing `constraints` field sent by OTP.
        // Either replace this by ProviderAuctionAck or update upstream.
        let buf = serde_json::to_vec(&json!({
          "provider_ref": provider_ref,
          "link_name": link_name,
          "constraints": constraints,
          "host_id": self.host_key.public_key(),
        }))
        .context("failed to encode reply")?;
        Ok(Some(buf.into()))
    }

    #[instrument(skip(self))]
    async fn fetch_actor(&self, actor_ref: &str) -> anyhow::Result<wasmcloud_runtime::Actor> {
        let actor = fetch_actor(&actor_ref, &self.host_config.oci_opts)
            .await
            .context("failed to fetch actor")?;
        let actor = wasmcloud_runtime::Actor::new(&self.runtime, actor)
            .context("failed to initialize actor")?;
        Ok(actor)
    }

    #[instrument(skip(self, payload))]
    async fn handle_stop(&self, payload: impl AsRef<[u8]>, host_id: &str) -> anyhow::Result<Bytes> {
        let StopHostCommand { timeout, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize stop command")?;

        debug!(?timeout, "stop host");

        self.heartbeat.abort();
        self.data_watch.abort();
        self.queue.abort();
        let deadline =
            timeout.and_then(|timeout| Instant::now().checked_add(Duration::from_millis(timeout)));
        self.stop_tx.send_replace(deadline);
        Ok(SUCCESS.into())
    }

    #[instrument(skip(self, payload))]
    async fn handle_scale_actor(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let ScaleActorCommand {
            actor_id,
            actor_ref,
            count,
            annotations,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor scale command")?;

        debug!(actor_id, actor_ref, count, "scale actor");

        let (actor_id, actor) = if actor_id.is_empty() {
            let actor = self.fetch_actor(&actor_ref).await?;
            (
                actor.claims().context("claims missing")?.subject.clone(),
                Some(actor),
            )
        } else {
            (actor_id, None)
        };

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        match (
            self.actors.write().await.entry(actor_id),
            NonZeroUsize::new(count.into()),
        ) {
            (hash_map::Entry::Vacant(_), None) => {}
            (hash_map::Entry::Vacant(entry), Some(count)) => {
                let actor = if let Some(actor) = actor {
                    actor
                } else {
                    self.fetch_actor(&actor_ref).await?
                };
                self.start_actor(entry, actor, actor_ref, count, host_id, annotations)
                    .await?;
            }
            (hash_map::Entry::Occupied(entry), None) => {
                self.stop_actor(entry, host_id).await?;
            }
            (hash_map::Entry::Occupied(entry), Some(count)) => {
                let actor = entry.get();
                let mut instances = actor.instances.write().await;
                let count = usize::from(count);
                let current = instances.values().map(Vec::len).sum();
                let claims = actor.pool.claims().context("claims missing")?;
                if let Some(delta) = count.checked_sub(current).and_then(NonZeroUsize::new) {
                    let mut delta = self
                        .instantiate_actor(
                            claims,
                            &annotations,
                            host_id,
                            &actor.image_ref,
                            delta,
                            actor.pool.clone(),
                            actor.handler.clone(),
                        )
                        .await
                        .context("failed to instantiate actor")?;
                    instances.entry(annotations).or_default().append(&mut delta);
                } else if let Some(delta) = current.checked_sub(count).and_then(NonZeroUsize::new) {
                    let mut remaining = current;
                    let mut delta = usize::from(delta);
                    for (annotations, instances) in instances.iter_mut() {
                        let Some(count) = NonZeroUsize::new(instances.len().min(delta)) else {
                            continue;
                        };
                        remaining = remaining
                            .checked_sub(count.into())
                            .context("invalid instance length")?;
                        delta = delta.checked_sub(count.into()).context("invalid delta")?;
                        self.uninstantiate_actor(
                            claims,
                            annotations,
                            host_id,
                            instances,
                            count,
                            remaining,
                        )
                        .await
                        .context("failed to uninstantiate actor")?;
                        if delta == 0 {
                            break;
                        }
                    }
                    if remaining == 0 {
                        drop(instances);
                        entry.remove();
                    }
                }
            }
        }
        Ok(SUCCESS.into())
    }

    #[instrument(skip(self))]
    async fn handle_launch_actor_task(
        &self,
        actor_ref: String,
        annotations: Option<HashMap<String, String>>,
        count: u16,
        host_id: &str,
    ) -> anyhow::Result<()> {
        debug!("launch actor");

        let actor = self.fetch_actor(&actor_ref).await?;
        let claims = actor.claims().context("claims missing")?;
        self.store_claims(claims.clone())
            .await
            .context("failed to store claims")?;

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        let Some(count) = NonZeroUsize::new(count.into()) else {
            // NOTE: This mimics OTP behavior
            self.publish_event(
                "actors_started",
                event::actors_started(claims, &annotations, host_id, 0usize, actor_ref),
            )
            .await?;
            return Ok(())
        };

        match self.actors.write().await.entry(claims.subject.clone()) {
            hash_map::Entry::Vacant(entry) => {
                if let Err(err) = self
                    .start_actor(
                        entry,
                        actor.clone(),
                        actor_ref.clone(),
                        count,
                        host_id,
                        annotations.clone(),
                    )
                    .await
                {
                    self.publish_event(
                        "actors_start_failed",
                        event::actors_start_failed(claims, &annotations, host_id, actor_ref, &err),
                    )
                    .await?;
                }
            }
            hash_map::Entry::Occupied(entry) => {
                let actor = entry.get();
                let mut instances = actor.instances.write().await;
                let claims = actor.pool.claims().context("claims missing")?;
                let mut delta = self
                    .instantiate_actor(
                        claims,
                        &annotations,
                        host_id,
                        &actor.image_ref,
                        count,
                        actor.pool.clone(),
                        actor.handler.clone(),
                    )
                    .await
                    .context("failed to instantiate actor")?;
                instances.entry(annotations).or_default().append(&mut delta);
            }
        }
        Ok(())
    }

    #[instrument(skip(self, payload))]
    async fn handle_launch_actor(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let StartActorCommand {
            actor_ref,
            annotations,
            count,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor launch command")?;

        let host_id = host_id.to_string();
        spawn(async move {
            if let Err(err) = self
                .handle_launch_actor_task(actor_ref.clone(), annotations, count, &host_id)
                .await
            {
                if let Err(err) = self
                    .publish_event(
                        "actor_start_failed",
                        event::actor_start_failed(actor_ref, &err),
                    )
                    .await
                {
                    error!("{err:#}");
                }
            }
        });
        Ok(SUCCESS.into())
    }

    #[instrument(skip(self, payload))]
    async fn handle_stop_actor(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let StopActorCommand {
            actor_ref,
            count,
            annotations,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor stop command")?;

        debug!(actor_ref, count, ?annotations, "stop actor");

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        match (
            self.actors.write().await.entry(actor_ref),
            NonZeroUsize::new(count.into()),
        ) {
            (hash_map::Entry::Occupied(entry), None) => {
                self.stop_actor(entry, host_id).await?;
            }
            (hash_map::Entry::Occupied(entry), Some(count)) => {
                let actor = entry.get();
                let claims = actor.pool.claims().context("claims missing")?;
                let mut instances = actor.instances.write().await;
                if let hash_map::Entry::Occupied(mut entry) = instances.entry(annotations.clone()) {
                    let instances = entry.get_mut();
                    let remaining = instances.len().saturating_sub(count.into());
                    self.uninstantiate_actor(
                        claims,
                        &annotations,
                        host_id,
                        instances,
                        count,
                        remaining,
                    )
                    .await
                    .context("failed to uninstantiate actor")?;
                    if remaining == 0 {
                        entry.remove();
                    }
                }
                if instances.len() == 0 {
                    drop(instances);
                    entry.remove();
                }
            }
            _ => {
                // NOTE: This mimics OTP behavior
                // TODO: What does OTP do?
            }
        }
        Ok(SUCCESS.into())
    }

    #[instrument(skip(self, payload))]
    async fn handle_update_actor(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let UpdateActorCommand {
            actor_id,
            annotations,
            new_actor_ref,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor update command")?;

        debug!(actor_id, new_actor_ref, ?annotations, "update actor");

        let actors = self.actors.write().await;
        let actor = actors.get(&actor_id).context("actor not found")?;
        let annotations = annotations.map(|annotations| annotations.into_iter().collect()); // convert from HashMap to BTreeMap
        let mut all_instances = actor.instances.write().await;
        let matching_instances = all_instances
            .get_mut(&annotations)
            .context("actor instances with matching annotations not found")?;
        let count =
            NonZeroUsize::new(matching_instances.len()).context("zero instances of actor found")?;

        let new_actor = self.fetch_actor(&new_actor_ref).await?;
        let new_claims = new_actor
            .claims()
            .context("claims missing from new actor")?;
        self.store_claims(new_claims.clone())
            .await
            .context("failed to store claims")?;
        let old_claims = actor
            .pool
            .claims()
            .context("claims missing from running actor")?;

        self.uninstantiate_actor(
            old_claims,
            &annotations,
            host_id,
            matching_instances,
            count,
            0,
        )
        .await
        .context("failed to uninstantiate running actor")?;

        let new_pool = ActorInstancePool::new(new_actor.clone(), Some(count));
        let mut new_instances = self
            .instantiate_actor(
                new_claims,
                &annotations,
                host_id,
                new_actor_ref,
                count,
                new_pool,
                actor.handler.clone(),
            )
            .await
            .context("failed to instantiate actor from new reference")?;
        all_instances
            .entry(annotations)
            .or_default()
            .append(&mut new_instances);

        Ok(SUCCESS.into())
    }

    #[instrument(skip(self))]
    async fn handle_launch_provider_task(
        &self,
        configuration: Option<String>,
        link_name: &str,
        provider_ref: &str,
        annotations: Option<HashMap<String, String>>,
        host_id: &str,
    ) -> anyhow::Result<()> {
        debug!("launch provider");

        let (path, claims) =
            crate::fetch_provider(provider_ref, link_name, &self.host_config.oci_opts)
                .await
                .context("failed to fetch provider")?;
        self.store_claims(claims.clone())
            .await
            .context("failed to store claims")?;

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        let mut providers = self.providers.write().await;
        let Provider { instances, .. } =
            providers.entry(claims.subject.clone()).or_insert(Provider {
                claims: claims.clone(),
                image_ref: provider_ref.into(),
                instances: HashMap::default(),
            });
        if let hash_map::Entry::Vacant(entry) = instances.entry(link_name.into()) {
            let id = Ulid::new();
            let async_nats::ServerInfo { host, port, .. } = self.nats.server_info();
            let invocation_seed = self
                .cluster_key
                .seed()
                .context("cluster key seed missing")?;
            let links = self.links.read().await;
            let link_definitions: Vec<_> = links
                .values()
                .filter(|ld| ld.provider_id == claims.subject && ld.link_name == link_name)
                .collect();
            let data = serde_json::to_vec(&json!({
                "host_id": self.host_key.public_key(),
                "lattice_rpc_prefix": self.host_config.lattice_prefix,
                "link_name": link_name,
                "lattice_rpc_user_jwt": "", // TODO: Support config
                "lattice_rpc_user_seed": "", // TODO: Support config
                "lattice_rpc_url": format!("{host}:{port}"),
                "lattice_rpc_tls": 0, // TODO: Support config
                "env_values": {},
                "instance_id": Uuid::from_u128(id.into()),
                "provider_key": claims.subject,
                "link_definitions": link_definitions,
                "config_json": configuration,
                "default_rpc_timeout_ms": 2000, // TODO: Support config
                "cluster_issuers": self.host_config.cluster_issuers.clone().unwrap_or_else(|| vec![self.cluster_key.public_key()]),
                "invocation_seed": invocation_seed,
                "js_domain": self.host_config.js_domain,
                // TODO: Set `structured_logging`
                // TODO: Set `log_level`
            }))
            .context("failed to serialize provider data")?;

            debug!(
                ?path,
                data = &*String::from_utf8_lossy(&data),
                "spawn provider process"
            );
            let mut child = process::Command::new(&path)
                .stdin(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("failed to spawn provider process")?;
            let mut stdin = child.stdin.take().context("failed to take stdin")?;
            stdin
                .write_all(STANDARD.encode(&data).as_bytes())
                .await
                .context("failed to write provider data")?;
            stdin
                .write_all(b"\r\n")
                .await
                .context("failed to write newline")?;
            stdin.shutdown().await.context("failed to close stdin")?;

            let child = spawn(async move {
                match child.wait().await {
                    Ok(status) => {
                        debug!("`{}` exited with `{status:?}`", path.display());
                    }
                    Err(e) => {
                        error!("failed to wait for `{}` to execute: {e}", path.display());
                    }
                }
            });
            self.publish_event(
                "provider_started",
                event::provider_started(
                    &claims,
                    &annotations,
                    Uuid::from_u128(id.into()),
                    host_id,
                    provider_ref,
                    link_name,
                ),
            )
            .await?;
            entry.insert(ProviderInstance {
                child,
                id,
                annotations,
            });
        } else {
            bail!("provider is already running")
        }
        Ok(())
    }

    #[instrument(skip(self, payload))]
    async fn handle_launch_provider(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let StartProviderCommand {
            configuration,
            link_name,
            provider_ref,
            annotations,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider launch command")?;
        let host_id = host_id.to_string();
        spawn(async move {
            if let Err(err) = self
                .handle_launch_provider_task(
                    configuration,
                    &link_name,
                    &provider_ref,
                    annotations,
                    &host_id,
                )
                .await
            {
                if let Err(err) = self
                    .publish_event(
                        "provider_start_failed",
                        event::provider_start_failed(provider_ref, link_name, &err),
                    )
                    .await
                {
                    error!("{err:#}");
                }
            }
        });
        Ok(SUCCESS.into())
    }

    #[instrument(skip(self, payload))]
    async fn handle_stop_provider(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let StopProviderCommand {
            annotations,
            contract_id,
            link_name,
            provider_ref,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider stop command")?;

        debug!(
            link_name,
            provider_ref,
            contract_id,
            ?annotations,
            "stop provider"
        );

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        let mut providers = self.providers.write().await;
        let hash_map::Entry::Occupied(mut entry) = providers.entry(provider_ref.clone()) else {
            return Ok(SUCCESS.into());
        };
        let provider = entry.get_mut();
        let instances = &mut provider.instances;
        if let hash_map::Entry::Occupied(entry) = instances.entry(link_name.clone()) {
            if entry.get().annotations == annotations {
                let ProviderInstance { id, child, .. } = entry.remove();

                // Send a request to the provider, requesting a graceful shutdown
                if let Ok(payload) = serde_json::to_vec(&json!({ "host_id": host_id })) {
                    if let Err(e) = self
                        .nats
                        .send_request(
                            format!(
                                "wasmbus.rpc.{}.{provider_ref}.{link_name}.shutdown",
                                self.host_config.lattice_prefix
                            ),
                            async_nats::Request::new()
                                .payload(payload.into())
                                .timeout(self.host_config.provider_shutdown_delay),
                        )
                        .await
                    {
                        warn!(?e, "Provider didn't gracefully shut down in time, shutting down forcefully");
                    }
                }

                child.abort();
                self.publish_event(
                    "provider_stopped",
                    event::provider_stopped(
                        &provider.claims,
                        &annotations,
                        Uuid::from_u128(id.into()),
                        host_id,
                        link_name,
                        "stop",
                    ),
                )
                .await?;
            }
        }
        if instances.is_empty() {
            entry.remove();
        }
        Ok(SUCCESS.into())
    }

    #[instrument(skip(self, _payload))]
    async fn handle_inventory(
        &self,
        _payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let actors = self.actors.read().await;
        let actors: Vec<_> = stream::iter(actors.iter())
            .filter_map(|(id, actor)| async move {
                let instances = actor.instances.read().await;
                let instances: Vec<_> = instances
                    .iter()
                    .flat_map(|(annotations, instances)| {
                        instances.iter().map(move |actor| {
                            let instance_id = Uuid::from_u128(actor.id.into()).to_string();
                            let revision = actor
                                .pool
                                .claims()
                                .and_then(|claims| claims.metadata.as_ref())
                                .and_then(|jwt::Actor { rev, .. }| *rev)
                                .unwrap_or_default();
                            let annotations = annotations
                                .as_ref()
                                .map(|annotations| annotations.clone().into_iter().collect());
                            wasmcloud_control_interface::ActorInstance {
                                annotations,
                                instance_id,
                                revision,
                            }
                        })
                    })
                    .collect();
                if instances.is_empty() {
                    return None;
                }
                let name = actor
                    .pool
                    .claims()
                    .and_then(|claims| claims.metadata.as_ref())
                    .and_then(|metadata| metadata.name.as_ref())
                    .cloned();
                Some(ActorDescription {
                    id: id.into(),
                    image_ref: Some(actor.image_ref.clone()),
                    instances,
                    name,
                })
            })
            .collect()
            .await;
        let providers = self.providers.read().await;
        let providers: Vec<_> = providers
            .iter()
            .filter_map(
                |(
                    id,
                    Provider {
                        claims,
                        instances,
                        image_ref,
                        ..
                    },
                )| {
                    let jwt::CapabilityProvider {
                        capid: contract_id,
                        name,
                        rev: revision,
                        ..
                    } = claims.metadata.as_ref()?;
                    Some(instances.iter().map(
                        move |(link_name, ProviderInstance { annotations, .. })| {
                            let annotations = annotations
                                .as_ref()
                                .map(|annotations| annotations.clone().into_iter().collect());
                            let revision = revision.unwrap_or_default();
                            ProviderDescription {
                                id: id.into(),
                                image_ref: Some(image_ref.clone()),
                                contract_id: contract_id.clone(),
                                link_name: link_name.into(),
                                name: name.clone(),
                                annotations,
                                revision,
                            }
                        },
                    ))
                },
            )
            .flatten()
            .collect();
        let buf = serde_json::to_vec(&HostInventory {
            host_id: self.host_key.public_key(),
            issuer: self.cluster_key.public_key(),
            labels: self.labels.clone(),
            friendly_name: self.friendly_name.clone(),
            actors,
            providers,
        })
        .context("failed to encode reply")?;
        Ok(buf.into())
    }

    #[instrument(skip(self))]
    async fn handle_claims(&self) -> anyhow::Result<Bytes> {
        // TODO: update control interface client to have a more specific type definition for
        // GetClaimsResponse, so we can re-use it here. Currently it's Vec<HashMap<String, String>>
        #[derive(Serialize)]
        struct ClaimsResponse {
            claims: Vec<StoredClaims>,
        }

        let claims: Vec<StoredClaims> = self.scan_latticedata(CLAIMS_PREFIX).await?;
        let resp = ClaimsResponse { claims };
        Ok(serde_json::to_vec(&resp)?.into())
    }

    #[instrument(skip(self))]
    async fn handle_links(&self) -> anyhow::Result<Bytes> {
        let links: Vec<LinkDefinition> = self.scan_latticedata(LINKDEF_PREFIX).await?;
        let resp = LinkDefinitionList { links };
        Ok(serde_json::to_vec(&resp)?.into())
    }

    async fn scan_latticedata<T: DeserializeOwned>(
        &self,
        key_prefix: &str,
    ) -> anyhow::Result<Vec<T>> {
        let filtered_keys = self
            .data
            .keys()
            .await
            .context("failed to scan lattice data keys")?
            .try_filter(|key| futures::future::ready(key.starts_with(key_prefix)))
            .try_collect::<Vec<String>>()
            .await
            .context("failed to collect lattice data keys")?;

        let futs = filtered_keys.into_iter().map(|key| self.data.get(key));
        let list: Vec<T> = futures::future::join_all(futs)
            .await
            .into_iter()
            .filter_map(|resp| {
                // TODO: we should probably handle when we get an error from NATS or encountering
                // malformed data in the bucket entries https://github.com/wasmCloud/wasmCloud/issues/509
                resp.ok()
                    .and_then(|bytes| bytes)
                    .and_then(|bytes| serde_json::from_slice::<T>(&bytes).ok())
            })
            .collect();

        Ok(list)
    }

    #[instrument(skip(self, payload))]
    async fn handle_linkdef_put(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        let payload = payload.as_ref();
        let LinkDefinition {
            actor_id,
            provider_id,
            link_name,
            contract_id,
            values,
            ..
        } = serde_json::from_slice(payload).context("failed to deserialize link definition")?;
        let id = linkdef_hash(&actor_id, &contract_id, &link_name);

        debug!(
            id,
            actor_id,
            provider_id,
            link_name,
            contract_id,
            ?values,
            "put link definition"
        );
        self.data
            .put(format!("LINKDEF_{id}"), Bytes::copy_from_slice(payload))
            .await
            .map_err(|e| anyhow!(e).context("failed to store link definition"))?;
        Ok(SUCCESS.into())
    }

    #[allow(unused)] // TODO: Remove once implemented
    #[instrument(skip(self, payload))]
    async fn handle_linkdef_del(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        let RemoveLinkDefinitionRequest {
            actor_id,
            ref link_name,
            contract_id,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize link definition deletion command")?;
        let id = linkdef_hash(&actor_id, &contract_id, link_name);

        debug!(
            id,
            actor_id, link_name, contract_id, "delete link definition"
        );
        self.data
            .delete(format!("LINKDEF_{id}"))
            .await
            .map_err(|e| anyhow!(e).context("failed to delete link definition"))?;
        Ok(SUCCESS.into())
    }

    #[allow(unused)] // TODO: Remove once implemented
    #[instrument(skip(self, payload))]
    async fn handle_registries_put(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        bail!("TODO")
    }

    #[instrument(skip(self, _payload))]
    async fn handle_ping_hosts(&self, _payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        let uptime = self.start_at.elapsed();
        let cluster_issuers = self
            .host_config
            .cluster_issuers
            .clone()
            .unwrap_or_else(|| vec![self.cluster_key.public_key()])
            .join(",");
        // TODO: Fill in the TODOs
        let buf = serde_json::to_vec(&json!({
          "id": self.host_key.public_key(),
          "issuer": self.cluster_key.public_key(),
          "labels": self.labels,
          "friendly_name": self.friendly_name,
          "uptime_seconds": uptime.as_secs(),
          "uptime_human": "TODO",
          "version": env!("CARGO_PKG_VERSION"),
          "cluster_issuers": cluster_issuers,
          "js_domain": self.host_config.js_domain,
          "ctl_host": "TODO",
          "prov_rpc_host": "TODO",
          "rpc_host": "TODO",
          "lattice_prefix": self.host_config.lattice_prefix,
        }))
        .context("failed to encode reply")?;
        Ok(buf.into())
    }

    #[instrument(skip(self))]
    async fn handle_message(
        self: Arc<Self>,
        async_nats::Message {
            subject,
            reply,
            payload,
            headers,
            status,
            description,
            ..
        }: async_nats::Message,
    ) {
        let mut parts = subject.split('.').skip(3); // skip `wasmbus.ctl.{prefix}`
        let res = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some("auction"), Some("actor"), None, None) => {
                self.handle_auction_actor(payload).await.map(Some)
            }
            (Some("auction"), Some("provider"), None, None) => {
                self.handle_auction_provider(payload).await
            }
            (Some("cmd"), Some(host_id), Some("la"), None) => Arc::clone(&self)
                .handle_launch_actor(payload, host_id)
                .await
                .map(Some),
            (Some("cmd"), Some(host_id), Some("lp"), None) => Arc::clone(&self)
                .handle_launch_provider(payload, host_id)
                .await
                .map(Some),
            (Some("cmd"), Some(host_id), Some("sa"), None) => {
                self.handle_stop_actor(payload, host_id).await.map(Some)
            }
            (Some("cmd"), Some(host_id), Some("scale"), None) => {
                self.handle_scale_actor(payload, host_id).await.map(Some)
            }
            (Some("cmd"), Some(host_id), Some("sp"), None) => {
                self.handle_stop_provider(payload, host_id).await.map(Some)
            }
            (Some("cmd"), Some(host_id), Some("stop"), None) => {
                self.handle_stop(payload, host_id).await.map(Some)
            }
            (Some("cmd"), Some(host_id), Some("upd"), None) => {
                self.handle_update_actor(payload, host_id).await.map(Some)
            }
            (Some("get"), Some(host_id), Some("inv"), None) => {
                self.handle_inventory(payload, host_id).await.map(Some)
            }
            (Some("get"), Some("claims"), None, None) => self.handle_claims().await.map(Some),
            (Some("get"), Some("links"), None, None) => self.handle_links().await.map(Some),
            (Some("linkdefs"), Some("put"), None, None) => {
                self.handle_linkdef_put(payload).await.map(Some)
            }
            (Some("linkdefs"), Some("del"), None, None) => {
                self.handle_linkdef_del(payload).await.map(Some)
            }
            (Some("registries"), Some("put"), None, None) => {
                self.handle_registries_put(payload).await.map(Some)
            }
            (Some("ping"), Some("hosts"), None, None) => {
                self.handle_ping_hosts(payload).await.map(Some)
            }
            _ => {
                error!("unsupported subject `{subject}`");
                return;
            }
        };
        if let Err(e) = &res {
            warn!("failed to handle `{subject}` request: {e:?}");
        }
        match (reply, res) {
            (Some(reply), Ok(Some(buf))) => {
                if let Err(e) = self.nats.publish(reply, buf).await {
                    error!("failed to publish success in response to `{subject}` request: {e:?}");
                }
            }
            (Some(reply), Err(e)) => {
                if let Err(e) = self
                    .nats
                    .publish(
                        reply,
                        format!(r#"{{"accepted":false,"error":"{e}"}}"#).into(),
                    )
                    .await
                {
                    error!("failed to publish error: {e:?}");
                }
            }
            _ => {}
        }
    }

    async fn store_claims<T>(&self, claims: T) -> anyhow::Result<()>
    where
        T: TryInto<StoredClaims, Error = anyhow::Error>,
    {
        let stored_claims: StoredClaims = claims.try_into()?;
        let key = format!("CLAIMS_{}", stored_claims.subject);

        let bytes = serde_json::to_vec(&stored_claims)
            .map_err(anyhow::Error::from)
            .context("failed to serialize claims")?
            .into();
        self.data
            .put(key, bytes)
            .await
            .context("failed to put claims")?;
        Ok(())
    }

    #[instrument(skip(self, id, value))]
    async fn process_linkdef_put(
        &self,
        id: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        let value = value.as_ref();
        let ref ld @ LinkDefinition {
            ref actor_id,
            ref provider_id,
            ref link_name,
            ref contract_id,
            ref values,
            ..
        } = serde_json::from_slice(value).context("failed to deserialize link definition")?;
        ensure!(
            id == linkdef_hash(actor_id, contract_id, link_name),
            "linkdef hash mismatch"
        );

        debug!(
            id,
            actor_id,
            provider_id,
            link_name,
            contract_id,
            ?values,
            "process link definition entry put"
        );

        let mut links = self.links.write().await;
        links.insert(id.to_string(), ld.clone());
        if let Some(actor) = self.actors.write().await.get_mut(actor_id) {
            let mut interfaces = actor.handler.interfaces.write().await;
            interfaces.insert(
                contract_id.clone(),
                WasmCloudEntity {
                    link_name: ld.link_name.clone(),
                    contract_id: ld.contract_id.clone(),
                    public_key: ld.provider_id.clone(),
                },
            );
        }

        self.publish_event(
            "linkdef_set",
            event::linkdef_set(id, actor_id, provider_id, link_name, contract_id, values),
        )
        .await?;

        let msgp = rmp_serde::to_vec(ld).context("failed to encode link definition")?;
        let lattice_prefix = &self.host_config.lattice_prefix;
        self.nats
            .publish(
                format!("wasmbus.rpc.{lattice_prefix}.{provider_id}.{link_name}.linkdefs.put",),
                msgp.into(),
            )
            .await
            .context("failed to publish link definition")?;
        Ok(())
    }

    #[instrument(skip(self, id, _value))]
    async fn process_linkdef_delete(
        &self,
        id: impl AsRef<str>,
        _value: impl AsRef<[u8]>,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();

        debug!(id, "process link definition entry deletion");

        let mut links = self.links.write().await;
        // NOTE: There is a race condition here, which occurs when `linkdefs.del`
        // is used before `data_watch` task has fully imported the current lattice,
        // but that command is deprecated, so assume it's fine
        let ref ld @ LinkDefinition {
            ref actor_id,
            ref provider_id,
            ref link_name,
            ref contract_id,
            ref values,
            ..
        } = links
            .remove(id)
            .context("attempt to remove a non-existent link")?;

        self.publish_event(
            "linkdef_deleted",
            event::linkdef_deleted(id, actor_id, provider_id, link_name, contract_id, values),
        )
        .await?;

        // TODO: Broadcast `linkdef_removed`

        let msgp = rmp_serde::to_vec(ld).context("failed to encode link definition")?;
        let lattice_prefix = &self.host_config.lattice_prefix;
        self.nats
            .publish(
                format!("wasmbus.rpc.{lattice_prefix}.{provider_id}.{link_name}.linkdefs.del",),
                msgp.into(),
            )
            .await
            .context("failed to publish link definition deletion")?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn process_entry(
        &self,
        async_nats::jetstream::kv::Entry {
            bucket,
            key,
            value,
            revision,
            delta,
            created,
            operation,
        }: async_nats::jetstream::kv::Entry,
    ) {
        use async_nats::jetstream::kv::Operation;

        let mut key_parts = key.split('_');
        let res = match (operation, key_parts.next(), key_parts.next()) {
            (Operation::Put, Some("LINKDEF"), Some(id)) => {
                self.process_linkdef_put(id, value).await
            }
            (Operation::Delete, Some("LINKDEF"), Some(id)) => {
                self.process_linkdef_delete(id, value).await
            }
            (Operation::Put, Some("CLAIMS"), Some(_pubkey)) => {
                // TODO https://github.com/wasmCloud/wasmCloud/issues/507
                Ok(())
            }
            (Operation::Delete, Some("CLAIMS"), Some(_pubkey)) => {
                // TODO https://github.com/wasmCloud/wasmCloud/issues/507
                Ok(())
            }
            _ => {
                error!(
                    bucket,
                    key,
                    revision,
                    delta,
                    ?created,
                    ?operation,
                    "unsupported KV bucket entry"
                );
                return;
            }
        };
        if let Err(error) = &res {
            warn!(?error, ?operation, bucket, "failed to process entry");
        }
    }
}

// TODO: use a better format https://github.com/wasmCloud/wasmCloud/issues/508
#[derive(Serialize, Deserialize)]
struct StoredClaims {
    call_alias: String,
    #[serde(rename = "caps")]
    capabilities: String,
    contract_id: String,
    #[serde(rename = "iss")]
    issuer: String,
    name: String,
    #[serde(rename = "rev")]
    revision: String,
    #[serde(rename = "sub")]
    subject: String,
    tags: String,
    version: String,
}

impl TryFrom<jwt::Claims<jwt::Actor>> for StoredClaims {
    type Error = anyhow::Error;

    fn try_from(claims: jwt::Claims<jwt::Actor>) -> Result<Self, Self::Error> {
        let jwt::Claims {
            issuer,
            subject,
            metadata,
            ..
        } = claims;

        let metadata = metadata.context("no metadata found on provider claims")?;

        Ok(StoredClaims {
            call_alias: metadata.call_alias.unwrap_or_default(),
            capabilities: metadata.caps.unwrap_or_default().join(","),
            contract_id: String::new(), // actors don't have a contract_id
            issuer,
            name: metadata.name.unwrap_or_default(),
            revision: metadata.rev.unwrap_or_default().to_string(),
            subject,
            tags: metadata.tags.unwrap_or_default().join(","),
            version: metadata.ver.unwrap_or_default(),
        })
    }
}

impl TryFrom<jwt::Claims<jwt::CapabilityProvider>> for StoredClaims {
    type Error = anyhow::Error;

    fn try_from(claims: jwt::Claims<jwt::CapabilityProvider>) -> Result<Self, Self::Error> {
        let jwt::Claims {
            issuer,
            subject,
            metadata,
            ..
        } = claims;

        let metadata = metadata.context("no metadata found on provider claims")?;

        Ok(StoredClaims {
            call_alias: String::new(),   // providers don't have a call alias
            capabilities: String::new(), // providers don't have a capabilities list
            contract_id: metadata.capid,
            issuer,
            name: metadata.name.unwrap_or_default(),
            revision: metadata.rev.unwrap_or_default().to_string(),
            subject,
            tags: String::new(), // providers don't have tags
            version: metadata.ver.unwrap_or_default(),
        })
    }
}
