/// wasmCloud host configuration
pub mod config;

pub use config::Host as HostConfig;

mod event;

use crate::{
    fetch_actor, socket_pair, OciConfig, PolicyAction, PolicyHostInfo, PolicyManager,
    PolicyRequestSource, PolicyRequestTarget, PolicyResponse, RegistryAuth, RegistryConfig,
    RegistryType,
};

use core::future::Future;
use core::num::NonZeroUsize;
use core::ops::RangeInclusive;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;

use std::collections::hash_map::{self, Entry};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::env::consts::{ARCH, FAMILY, OS};
use std::io::Cursor;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::{BufMut, Bytes, BytesMut};
use cloudevents::{EventBuilder, EventBuilderV10};
use futures::stream::{AbortHandle, Abortable};
use futures::{join, stream, try_join, FutureExt, Stream, StreamExt, TryStreamExt};
use nkeys::{KeyPair, KeyPairType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{empty, stderr, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{interval_at, Instant};
use tokio::{process, select, spawn};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn};
use ulid::Ulid;
use uuid::Uuid;
use wascap::{jwt, prelude::ClaimsBuilder};
use wasmcloud_control_interface::{
    ActorAuctionAck, ActorAuctionRequest, ActorDescription, HostInventory, LinkDefinition,
    LinkDefinitionList, ProviderAuctionRequest, ProviderDescription, RegistryCredential,
    RegistryCredentialMap, RemoveLinkDefinitionRequest, ScaleActorCommand, StartActorCommand,
    StartProviderCommand, StopActorCommand, StopHostCommand, StopProviderCommand,
    UpdateActorCommand,
};
use wasmcloud_core::chunking::{ChunkEndpoint, CHUNK_RPC_EXTRA_TIME, CHUNK_THRESHOLD_BYTES};
use wasmcloud_core::{
    HealthCheckResponse, HostData, Invocation, InvocationResponse, OtelConfig, WasmCloudEntity,
};
use wasmcloud_runtime::capability::logging::logging;
use wasmcloud_runtime::capability::{
    blobstore, messaging, ActorIdentifier, Blobstore, Bus, IncomingHttp, KeyValueAtomic,
    KeyValueReadWrite, Logging, Messaging, TargetEntity, TargetInterface,
};
use wasmcloud_runtime::{ActorInstancePool, Runtime};
use wasmcloud_tracing::context::TraceContextInjector;

const SUCCESS: &str = r#"{"accepted":true,"error":""}"#;

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
        topic_prefix: &str,
        lattice_prefix: &str,
        cluster_key: &KeyPair,
        host_key: &KeyPair,
    ) -> anyhow::Result<Self> {
        let host_id = host_key.public_key();
        let (registries, pings, links, queries, auction, commands, inventory) = try_join!(
            nats.subscribe(format!("{topic_prefix}.{lattice_prefix}.registries.put",)),
            nats.subscribe(format!("{topic_prefix}.{lattice_prefix}.ping.hosts",)),
            nats.subscribe(format!("{topic_prefix}.{lattice_prefix}.linkdefs.*",)),
            nats.subscribe(format!("{topic_prefix}.{lattice_prefix}.get.*",)),
            nats.subscribe(format!("{topic_prefix}.{lattice_prefix}.auction.>",)),
            nats.subscribe(format!("{topic_prefix}.{lattice_prefix}.cmd.{host_id}.*",)),
            nats.subscribe(format!("{topic_prefix}.{lattice_prefix}.get.{host_id}.inv",)),
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
    chunk_endpoint: ChunkEndpoint,
    /// Cluster issuers that this actor should accept invocations from
    valid_issuers: Vec<String>,
    policy_manager: Arc<PolicyManager>,
    actor_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::Actor>>>>, // TODO: use a single map once Claims is an enum
    provider_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::CapabilityProvider>>>>,
}

#[derive(Clone, Debug)]
struct Handler {
    nats: async_nats::Client,
    lattice_prefix: String,
    cluster_key: Arc<KeyPair>,
    host_key: Arc<KeyPair>,
    claims: jwt::Claims<jwt::Actor>,
    origin: WasmCloudEntity,
    // package -> target -> entity
    links: Arc<RwLock<HashMap<String, HashMap<String, WasmCloudEntity>>>>,
    targets: Arc<RwLock<HashMap<TargetInterface, TargetEntity>>>,
    aliases: Arc<RwLock<HashMap<String, WasmCloudEntity>>>,
    chunk_endpoint: ChunkEndpoint,
}

#[instrument]
async fn resolve_target(
    target: Option<&TargetEntity>,
    links: Option<&HashMap<String, WasmCloudEntity>>,
    aliases: &HashMap<String, WasmCloudEntity>,
) -> anyhow::Result<WasmCloudEntity> {
    const DEFAULT_LINK_NAME: &str = "default";

    trace!("resolve target");

    let target = match target {
        None => links
            .and_then(|targets| targets.get(DEFAULT_LINK_NAME))
            .context("link not found")?
            .clone(),
        Some(TargetEntity::Link(link_name)) => links
            .and_then(|targets| targets.get(link_name.as_deref().unwrap_or(DEFAULT_LINK_NAME)))
            .context("link not found")?
            .clone(),
        Some(TargetEntity::Actor(ActorIdentifier::Key(key))) => WasmCloudEntity {
            public_key: key.public_key(),
            ..Default::default()
        },
        Some(TargetEntity::Actor(ActorIdentifier::Alias(alias))) => aliases
            .get(alias)
            .context("unknown actor call alias")?
            .clone(),
    };
    Ok(target)
}

impl Handler {
    #[instrument(skip(self, operation, request))]
    async fn call_operation_with_payload(
        &self,
        target: Option<&TargetEntity>,
        operation: impl Into<String>,
        request: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, String>> {
        let links = self.links.read().await;
        let aliases = self.aliases.read().await;
        let operation = operation.into();
        let (package, _) = operation
            .rsplit_once('/')
            .context("failed to parse operation")?;
        let inv_target = resolve_target(target, links.get(package), &aliases).await?;
        let needs_chunking = request.len() > CHUNK_THRESHOLD_BYTES;
        let injector = TraceContextInjector::default_with_span();
        let headers = injector_to_headers(&injector);
        let mut invocation = Invocation::new(
            &self.cluster_key,
            &self.host_key,
            self.origin.clone(),
            inv_target,
            operation,
            request,
            injector.into(),
        )?;

        // Validate that the actor has the capability to call the target
        ensure_actor_capability(
            self.claims.metadata.as_ref(),
            &invocation.target.contract_id,
        )?;

        if needs_chunking {
            self.chunk_endpoint
                .chunkify(&invocation.id, Cursor::new(invocation.msg))
                .await
                .context("failed to chunk invocation")?;
            invocation.msg = vec![];
        }

        let payload =
            rmp_serde::to_vec_named(&invocation).context("failed to encode invocation")?;
        let topic = match target {
            None | Some(TargetEntity::Link(_)) => format!(
                "wasmbus.rpc.{}.{}.{}",
                self.lattice_prefix, invocation.target.public_key, invocation.target.link_name,
            ),
            Some(TargetEntity::Actor(_)) => format!(
                "wasmbus.rpc.{}.{}",
                self.lattice_prefix, invocation.target.public_key
            ),
        };

        let timeout = needs_chunking.then_some(CHUNK_RPC_EXTRA_TIME); // TODO: add rpc_nats timeout
        let request = async_nats::Request::new()
            .payload(payload.into())
            .timeout(timeout)
            .headers(headers); // TODO: remove headers once all providers are built off the new SDK, which parses the trace context in the invocation
        let res = self
            .nats
            .send_request(topic, request)
            .await
            .context("failed to publish on NATS topic")?;

        let InvocationResponse {
            invocation_id,
            mut msg,
            content_length,
            error,
            ..
        } = rmp_serde::from_slice(&res.payload).context("failed to decode invocation response")?;
        ensure!(invocation_id == invocation.id, "invocation ID mismatch");

        let resp_length =
            usize::try_from(content_length).context("content length does not fit in usize")?;
        if resp_length > CHUNK_THRESHOLD_BYTES {
            msg = self
                .chunk_endpoint
                .get_unchunkified_response(&invocation_id)
                .await
                .context("failed to dechunk response")?;
        } else {
            ensure!(resp_length == msg.len(), "message size mismatch");
        }

        if let Some(error) = error {
            Ok(Err(error))
        } else {
            Ok(Ok(msg))
        }
    }

    #[instrument(skip(self, operation, request))]
    async fn call_operation(
        &self,
        target: Option<&TargetEntity>,
        operation: impl Into<String>,
        request: &impl Serialize,
    ) -> anyhow::Result<Vec<u8>> {
        let request = rmp_serde::to_vec_named(request).context("failed to encode request")?;
        self.call_operation_with_payload(target, operation, request)
            .await
            .context("failed to call target entity")?
            .map_err(|err| anyhow!(err).context("call failed"))
    }
}

/// Decode provider response accounting for the custom wasmbus-rpc encoding format
fn decode_provider_response<T>(buf: impl AsRef<[u8]>) -> anyhow::Result<T>
where
    for<'a> T: Deserialize<'a>,
{
    let buf = buf.as_ref();
    match buf.split_first() {
        Some((0x7f, _)) => bail!("CBOR responses are not supported"),
        Some((0xc1, buf)) => rmp_serde::from_slice(buf),
        _ => rmp_serde::from_slice(buf),
    }
    .context("failed to decode response")
}

fn decode_empty_provider_response(buf: impl AsRef<[u8]>) -> anyhow::Result<()> {
    let buf = buf.as_ref();
    if buf.is_empty() {
        Ok(())
    } else {
        decode_provider_response(buf)
    }
}

#[async_trait]
impl Blobstore for Handler {
    #[instrument]
    async fn create_container(&self, name: &str) -> anyhow::Result<()> {
        let targets = self.targets.read().await;
        self.call_operation(
            targets.get(&TargetInterface::WasiBlobstoreBlobstore),
            "wasmcloud:blobstore/Blobstore.CreateContainer",
            &name,
        )
        .await
        .and_then(decode_empty_provider_response)
    }

    #[instrument]
    async fn container_exists(&self, name: &str) -> anyhow::Result<bool> {
        let targets = self.targets.read().await;
        self.call_operation(
            targets.get(&TargetInterface::WasiBlobstoreBlobstore),
            "wasmcloud:blobstore/Blobstore.ContainerExists",
            &name,
        )
        .await
        .and_then(decode_provider_response)
    }

    #[instrument]
    async fn delete_container(&self, name: &str) -> anyhow::Result<()> {
        let targets = self.targets.read().await;
        self.call_operation(
            targets.get(&TargetInterface::WasiBlobstoreBlobstore),
            "wasmcloud:blobstore/Blobstore.DeleteContainer",
            &name,
        )
        .await
        .and_then(decode_empty_provider_response)
    }

    #[instrument]
    async fn container_info(
        &self,
        name: &str,
    ) -> anyhow::Result<blobstore::container::ContainerMetadata> {
        let targets = self.targets.read().await;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasiBlobstoreBlobstore),
                "wasmcloud:blobstore/Blobstore.GetContainerInfo",
                &name,
            )
            .await?;
        let wasmcloud_compat::blobstore::ContainerMetadata {
            container_id: name,
            created_at,
        } = decode_provider_response(res)?;
        let created_at = created_at
            .map(|wasmcloud_compat::Timestamp { sec, .. }| sec.try_into())
            .transpose()
            .context("timestamp seconds do not fit in `u64`")?;
        Ok(blobstore::container::ContainerMetadata {
            name,
            created_at: created_at.unwrap_or_default(),
        })
    }

    #[instrument]
    async fn get_data(
        &self,
        container: &str,
        name: String,
        range: RangeInclusive<u64>,
    ) -> anyhow::Result<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)> {
        let targets = self.targets.read().await;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasiBlobstoreBlobstore),
                "wasmcloud:blobstore/Blobstore.GetObject",
                &wasmcloud_compat::blobstore::GetObjectRequest {
                    object_id: name.clone(),
                    container_id: container.into(),
                    range_start: Some(*range.start()),
                    range_end: Some(*range.end()),
                },
            )
            .await?;
        let wasmcloud_compat::blobstore::GetObjectResponse {
            success,
            error,
            initial_chunk,
            ..
        } = decode_provider_response(res)?;
        match (success, initial_chunk, error) {
            (_, _, Some(err)) => Err(anyhow!(err).context("failed to get object response")),
            (false, _, None) => bail!("failed to get object response"),
            (true, None, None) => Ok((Box::new(empty()), 0)),
            (
                true,
                Some(wasmcloud_compat::blobstore::Chunk {
                    object_id,
                    container_id,
                    bytes,
                    ..
                }),
                None,
            ) => {
                ensure!(object_id == name);
                ensure!(container_id == container);
                let size = bytes
                    .len()
                    .try_into()
                    .context("value size does not fit in `u64`")?;
                Ok((Box::new(Cursor::new(bytes)), size))
            }
        }
    }

    #[instrument]
    async fn has_object(&self, container: &str, name: String) -> anyhow::Result<bool> {
        let targets = self.targets.read().await;
        self.call_operation(
            targets.get(&TargetInterface::WasiBlobstoreBlobstore),
            "wasmcloud:blobstore/Blobstore.ObjectExists",
            &wasmcloud_compat::blobstore::ContainerObject {
                container_id: container.into(),
                object_id: name,
            },
        )
        .await
        .and_then(decode_provider_response)
    }

    #[instrument(skip(value))]
    async fn write_data(
        &self,
        container: &str,
        name: String,
        mut value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()> {
        let targets = self.targets.read().await;
        let mut bytes = Vec::new();
        value
            .read_to_end(&mut bytes)
            .await
            .context("failed to read bytes")?;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasiBlobstoreBlobstore),
                "wasmcloud:blobstore/Blobstore.PutObject",
                &wasmcloud_compat::blobstore::PutObjectRequest {
                    chunk: wasmcloud_compat::blobstore::Chunk {
                        object_id: name,
                        container_id: container.into(),
                        bytes,
                        offset: 0,
                        is_last: true,
                    },
                    ..Default::default()
                },
            )
            .await?;
        let wasmcloud_compat::blobstore::PutObjectResponse { stream_id } =
            decode_provider_response(res)?;
        ensure!(
            stream_id.is_none(),
            "provider returned an unexpected stream ID"
        );
        Ok(())
    }

    #[instrument]
    async fn delete_objects(&self, container: &str, names: Vec<String>) -> anyhow::Result<()> {
        let targets = self.targets.read().await;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasiBlobstoreBlobstore),
                "wasmcloud:blobstore/Blobstore.RemoveObjects",
                &wasmcloud_compat::blobstore::RemoveObjectsRequest {
                    container_id: container.into(),
                    objects: names,
                },
            )
            .await?;
        for wasmcloud_compat::blobstore::ItemResult {
            key,
            success,
            error,
        } in decode_provider_response::<Vec<_>>(res)?
        {
            if let Some(err) = error {
                bail!(err)
            }
            ensure!(success, "failed to delete object `{key}`");
        }
        Ok(())
    }

    #[instrument]
    async fn list_objects(
        &self,
        container: &str,
    ) -> anyhow::Result<Box<dyn Stream<Item = anyhow::Result<String>> + Sync + Send + Unpin>> {
        let targets = self.targets.read().await;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasiBlobstoreBlobstore),
                "wasmcloud:blobstore/Blobstore.ListObjects",
                &wasmcloud_compat::blobstore::ListObjectsRequest {
                    container_id: container.into(),
                    max_items: Some(u32::MAX),
                    ..Default::default()
                },
            )
            .await?;
        let wasmcloud_compat::blobstore::ListObjectsResponse {
            objects,
            is_last,
            continuation,
        } = decode_provider_response(res)?;
        ensure!(is_last);
        ensure!(continuation.is_none(), "chunked responses not supported");
        Ok(Box::new(stream::iter(objects.into_iter().map(
            |wasmcloud_compat::blobstore::ObjectMetadata { object_id, .. }| Ok(object_id),
        ))))
    }

    #[instrument]
    async fn object_info(
        &self,
        container: &str,
        name: String,
    ) -> anyhow::Result<blobstore::container::ObjectMetadata> {
        let targets = self.targets.read().await;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasiBlobstoreBlobstore),
                "wasmcloud:blobstore/Blobstore.GetObjectInfo",
                &name,
            )
            .await?;
        let wasmcloud_compat::blobstore::ObjectMetadata {
            object_id,
            container_id,
            content_length,
            ..
        } = decode_provider_response(res)?;
        Ok(blobstore::container::ObjectMetadata {
            name: object_id,
            container: container_id,
            size: content_length,
            created_at: 0,
        })
    }
}

#[async_trait]
impl Bus for Handler {
    #[instrument(skip(self))]
    async fn identify_wasmbus_target(
        &self,
        binding: &str,
        namespace: &str,
    ) -> anyhow::Result<TargetEntity> {
        let links = self.links.read().await;
        if links
            .get(namespace)
            .map(|bindings| bindings.contains_key(binding))
            .unwrap_or_default()
        {
            return Ok(TargetEntity::Link(Some(binding.into())));
        }
        Ok(TargetEntity::Actor(namespace.into()))
    }

    #[instrument(skip(self))]
    async fn set_target(
        &self,
        target: Option<TargetEntity>,
        interfaces: Vec<TargetInterface>,
    ) -> anyhow::Result<()> {
        let mut targets = self.targets.write().await;
        if let Some(target) = target {
            for interface in interfaces {
                targets.insert(interface, target.clone());
            }
        } else {
            for interface in interfaces {
                targets.remove(&interface);
            }
        }
        Ok(())
    }

    #[instrument(skip(self))]
    async fn call(
        &self,
        target: Option<TargetEntity>,
        operation: String,
    ) -> anyhow::Result<(
        Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
        Box<dyn AsyncWrite + Sync + Send + Unpin>,
        Box<dyn AsyncRead + Sync + Send + Unpin>,
    )> {
        let (mut req_r, req_w) = socket_pair()?;
        let (res_r, mut res_w) = socket_pair()?;

        let links = Arc::clone(&self.links);
        let aliases = Arc::clone(&self.aliases);
        let nats = self.nats.clone();
        let chunk_endpoint = self.chunk_endpoint.clone();
        let lattice_prefix = self.lattice_prefix.clone();
        let origin = self.origin.clone();
        let cluster_key = self.cluster_key.clone();
        let host_key = self.host_key.clone();
        let claims_metadata = self.claims.metadata.clone();
        Ok((
            async move {
                // TODO: Stream data
                let mut request = vec![];
                req_r
                    .read_to_end(&mut request)
                    .await
                    .context("failed to read request")
                    .map_err(|e| e.to_string())?;
                let links = links.read().await;
                let aliases = aliases.read().await;
                let (package, _) = operation
                    .rsplit_once('/')
                    .context("failed to parse operation")
                    .map_err(|e| e.to_string())?;
                let inv_target = resolve_target(target.as_ref(), links.get(package), &aliases)
                    .await
                    .map_err(|e| e.to_string())?;
                let needs_chunking = request.len() > CHUNK_THRESHOLD_BYTES;
                let injector = TraceContextInjector::default_with_span();
                let headers = injector_to_headers(&injector);
                let mut invocation = Invocation::new(
                    &cluster_key,
                    &host_key,
                    origin,
                    inv_target,
                    operation,
                    request,
                    injector.into(),
                )
                .map_err(|e| e.to_string())?;

                // Validate that the actor has the capability to call the target
                ensure_actor_capability(claims_metadata.as_ref(), &invocation.target.contract_id)
                    .map_err(|e| e.to_string())?;

                if needs_chunking {
                    chunk_endpoint
                        .chunkify(&invocation.id, Cursor::new(invocation.msg))
                        .await
                        .context("failed to chunk invocation")
                        .map_err(|e| e.to_string())?;
                    invocation.msg = vec![];
                }

                let payload = rmp_serde::to_vec_named(&invocation)
                    .context("failed to encode invocation")
                    .map_err(|e| e.to_string())?;
                let topic = match target {
                    None | Some(TargetEntity::Link(_)) => format!(
                        "wasmbus.rpc.{lattice_prefix}.{}.{}",
                        invocation.target.public_key, invocation.target.link_name,
                    ),
                    Some(TargetEntity::Actor(_)) => format!(
                        "wasmbus.rpc.{lattice_prefix}.{}",
                        invocation.target.public_key
                    ),
                };

                let timeout = needs_chunking.then_some(CHUNK_RPC_EXTRA_TIME); // TODO: add rpc_nats timeout
                let request = async_nats::Request::new()
                    .payload(payload.into())
                    .timeout(timeout)
                    .headers(headers); // TODO: remove headers once all providers are built off the new SDK, which parses the trace context in the invocation
                let res = nats
                    .send_request(topic, request)
                    .await
                    .context("failed to call provider")
                    .map_err(|e| e.to_string())?;

                let InvocationResponse {
                    invocation_id,
                    mut msg,
                    content_length,
                    error,
                    ..
                } = rmp_serde::from_slice(&res.payload)
                    .context("failed to decode invocation response")
                    .map_err(|e| e.to_string())?;
                if invocation_id != invocation.id {
                    return Err("invocation ID mismatch".into());
                }

                let resp_length = usize::try_from(content_length)
                    .context("content length does not fit in usize")
                    .map_err(|e| e.to_string())?;
                if resp_length > CHUNK_THRESHOLD_BYTES {
                    msg = chunk_endpoint
                        .get_unchunkified_response(&invocation_id)
                        .await
                        .context("failed to dechunk response")
                        .map_err(|e| e.to_string())?;
                } else if resp_length != msg.len() {
                    return Err("message size mismatch".into());
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

    #[instrument(skip(self, request))]
    async fn call_sync(
        &self,
        target: Option<TargetEntity>,
        operation: String,
        request: Vec<u8>,
    ) -> anyhow::Result<Vec<u8>> {
        self.call_operation_with_payload(target.as_ref(), operation, request)
            .await
            .context("failed to call linked provider")?
            .map_err(|e| anyhow!(e).context("provider call failed"))
    }
}

#[async_trait]
impl KeyValueAtomic for Handler {
    #[instrument(skip(self))]
    async fn increment(&self, bucket: &str, key: String, delta: u64) -> anyhow::Result<u64> {
        const METHOD: &str = "wasmcloud:keyvalue/KeyValue.Increment";
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let targets = self.targets.read().await;
        let value = delta.try_into().context("delta does not fit in `i32`")?;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasiKeyvalueAtomic),
                METHOD,
                &wasmcloud_compat::keyvalue::IncrementRequest { key, value },
            )
            .await?;
        let new: i32 = decode_provider_response(res)?;
        let new = new.try_into().context("result does not fit in `u64`")?;
        Ok(new)
    }

    #[allow(unused)] // TODO: Implement https://github.com/wasmCloud/wasmCloud/issues/457
    #[instrument(skip(self))]
    async fn compare_and_swap(
        &self,
        bucket: &str,
        key: String,
        old: u64,
        new: u64,
    ) -> anyhow::Result<bool> {
        bail!("not supported")
    }
}

#[async_trait]
impl KeyValueReadWrite for Handler {
    #[instrument(skip(self))]
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)> {
        const METHOD: &str = "wasmcloud:keyvalue/KeyValue.Get";
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let targets = self.targets.read().await;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasiKeyvalueReadwrite),
                METHOD,
                &key,
            )
            .await?;
        let wasmcloud_compat::keyvalue::GetResponse { value, exists } =
            decode_provider_response(res)?;
        if !exists {
            bail!("key not found")
        }
        let size = value
            .len()
            .try_into()
            .context("value size does not fit in `u64`")?;
        Ok((Box::new(Cursor::new(value)), size))
    }

    #[instrument(skip(self, value))]
    async fn set(
        &self,
        bucket: &str,
        key: String,
        mut value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()> {
        const METHOD: &str = "wasmcloud:keyvalue/KeyValue.Set";
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let mut buf = String::new();
        value
            .read_to_string(&mut buf)
            .await
            .context("failed to read value")?;
        let targets = self.targets.read().await;
        self.call_operation(
            targets.get(&TargetInterface::WasiKeyvalueReadwrite),
            METHOD,
            &wasmcloud_compat::keyvalue::SetRequest {
                key,
                value: buf,
                expires: 0,
            },
        )
        .await
        .and_then(decode_empty_provider_response)
    }

    #[instrument(skip(self))]
    async fn delete(&self, bucket: &str, key: String) -> anyhow::Result<()> {
        const METHOD: &str = "wasmcloud:keyvalue/KeyValue.Del";
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let targets = self.targets.read().await;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasiKeyvalueReadwrite),
                METHOD,
                &key,
            )
            .await?;
        let deleted: bool = decode_provider_response(res)?;
        ensure!(deleted, "key not found");
        Ok(())
    }

    #[instrument(skip(self))]
    async fn exists(&self, bucket: &str, key: String) -> anyhow::Result<bool> {
        const METHOD: &str = "wasmcloud:keyvalue/KeyValue.Contains";
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let targets = self.targets.read().await;
        self.call_operation(
            targets.get(&TargetInterface::WasiKeyvalueReadwrite),
            METHOD,
            &key,
        )
        .await
        .and_then(decode_provider_response)
    }
}

#[async_trait]
impl Logging for Handler {
    #[instrument(skip_all)]
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        ensure_actor_capability(self.claims.metadata.as_ref(), wascap::caps::LOGGING)?;
        match level {
            logging::Level::Trace => {
                tracing::event!(
                    tracing::Level::TRACE,
                    actor_id = self.claims.subject,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Debug => {
                tracing::event!(
                    tracing::Level::DEBUG,
                    actor_id = self.claims.subject,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Info => {
                tracing::event!(
                    tracing::Level::INFO,
                    actor_id = self.claims.subject,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Warn => {
                tracing::event!(
                    tracing::Level::WARN,
                    actor_id = self.claims.subject,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Error => {
                tracing::event!(
                    tracing::Level::ERROR,
                    actor_id = self.claims.subject,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Critical => {
                tracing::event!(
                    tracing::Level::ERROR,
                    actor_id = self.claims.subject,
                    ?level,
                    context,
                    "{message}"
                );
            }
        };
        Ok(())
    }
}

#[async_trait]
impl Messaging for Handler {
    #[instrument(skip(self, body))]
    async fn request(
        &self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> anyhow::Result<messaging::types::BrokerMessage> {
        const METHOD: &str = "wasmcloud:messaging/Messaging.Request";

        let timeout_ms = timeout
            .as_millis()
            .try_into()
            .context("timeout milliseconds do not fit in `u32`")?;
        let targets = self.targets.read().await;
        let res = self
            .call_operation(
                targets.get(&TargetInterface::WasmcloudMessagingConsumer),
                METHOD,
                &wasmcloud_compat::messaging::RequestMessage {
                    subject,
                    body: body.unwrap_or_default(),
                    timeout_ms,
                },
            )
            .await?;
        let wasmcloud_compat::messaging::ReplyMessage {
            subject,
            reply_to,
            body,
        } = decode_provider_response(res)?;
        Ok(messaging::types::BrokerMessage {
            subject,
            reply_to,
            body: Some(body),
        })
    }

    #[instrument(skip(self, body))]
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

    #[instrument(skip_all)]
    async fn publish(
        &self,
        messaging::types::BrokerMessage {
            subject,
            reply_to,
            body,
        }: messaging::types::BrokerMessage,
    ) -> anyhow::Result<()> {
        const METHOD: &str = "wasmcloud:messaging/Messaging.Publish";
        let targets = self.targets.read().await;
        self.call_operation(
            targets.get(&TargetInterface::WasmcloudMessagingConsumer),
            METHOD,
            &wasmcloud_compat::messaging::PubMessage {
                subject,
                reply_to,
                body: body.unwrap_or_default(),
            },
        )
        .await
        .and_then(decode_empty_provider_response)
    }
}

impl ActorInstance {
    #[instrument(skip(self, msg))]
    async fn handle_invocation(
        &self,
        contract_id: &str,
        operation: &str,
        msg: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, String>> {
        // Validate that the actor has the capability to receive the invocation
        ensure_actor_capability(self.handler.claims.metadata.as_ref(), contract_id)?;

        let mut instance = self
            .pool
            .instantiate(self.runtime.clone())
            .await
            .context("failed to instantiate actor")?;
        instance
            .stderr(stderr())
            .await
            .context("failed to set stderr")?
            .blobstore(Arc::new(self.handler.clone()))
            .bus(Arc::new(self.handler.clone()))
            .keyvalue_atomic(Arc::new(self.handler.clone()))
            .keyvalue_readwrite(Arc::new(self.handler.clone()))
            .logging(Arc::new(self.handler.clone()))
            .messaging(Arc::new(self.handler.clone()));
        #[allow(clippy::single_match_else)] // TODO: Remove once more interfaces supported
        match (contract_id, operation) {
            ("wasmcloud:httpserver", "HttpServer.HandleRequest") => {
                let req: wasmcloud_compat::HttpRequest =
                    rmp_serde::from_slice(&msg).context("failed to decode HTTP request")?;
                let req = http::Request::try_from(req).context("failed to convert request")?;
                let res = match wasmcloud_runtime::ActorInstance::from(instance)
                    .into_incoming_http()
                    .await
                    .context("failed to instantiate `wasi:http/incoming-handler`")?
                    .handle(req.map(|body| -> Box<dyn AsyncRead + Send + Sync + Unpin> {
                        Box::new(Cursor::new(body))
                    }))
                    .await
                {
                    Ok(res) => res,
                    Err(err) => return Ok(Err(format!("{err:#}"))),
                };
                let res = wasmcloud_compat::HttpResponse::from_http(res)
                    .await
                    .context("failed to convert response")?;
                let res = rmp_serde::to_vec_named(&res).context("failed to encode response")?;
                Ok(Ok(res))
            }
            _ => {
                let res = AsyncBytesMut::default();
                match instance
                    .call(operation, Cursor::new(msg), res.clone())
                    .await
                    .context("failed to call actor")?
                {
                    Ok(()) => {
                        let res = res.try_into().context("failed to unwrap bytes")?;
                        Ok(Ok(res))
                    }
                    Err(e) => Ok(Err(e)),
                }
            }
        }
    }

    #[instrument(skip_all)]
    async fn handle_call(&self, invocation: Invocation) -> anyhow::Result<(Vec<u8>, u64)> {
        debug!(?invocation.origin, ?invocation.target, invocation.operation, "validate actor invocation");
        invocation.validate_antiforgery(&self.valid_issuers)?;

        let content_length: usize = invocation
            .content_length
            .try_into()
            .context("failed to convert content_length to usize")?;
        let inv_msg = if content_length > CHUNK_THRESHOLD_BYTES {
            debug!(inv_id = invocation.id, "dechunking invocation");
            self.chunk_endpoint.get_unchunkified(&invocation.id).await?
        } else {
            invocation.msg
        };

        debug!(?invocation.origin, ?invocation.target, invocation.operation, "handle actor invocation");

        let source_public_key = invocation.origin.public_key;
        // actors don't have a contract_id
        let source = if invocation.origin.contract_id.is_empty() {
            let actor_claims = self.actor_claims.read().await;
            let claims = actor_claims
                .get(&source_public_key)
                .cloned()
                .context("failed to look up claims for origin")?;
            PolicyRequestSource::from(claims)
        } else {
            let provider_claims = self.provider_claims.read().await;
            let claims = provider_claims
                .get(&source_public_key)
                .cloned()
                .context("failed to look up claims for origin")?;
            let mut source = PolicyRequestSource::from(claims);
            source.link_name = Some(invocation.origin.link_name.clone());
            source
        };

        // actors don't have a contract_id
        let target_public_key = invocation.target.public_key;
        let target = if invocation.target.contract_id.is_empty() {
            let actor_claims = self.actor_claims.read().await;
            let claims = actor_claims
                .get(&target_public_key)
                .cloned()
                .context("failed to look up claims for target")?;
            PolicyRequestTarget::from(claims)
        } else {
            let provider_claims = self.provider_claims.read().await;
            let claims = provider_claims
                .get(&target_public_key)
                .cloned()
                .context("failed to look up claims for target")?;
            let mut target = PolicyRequestTarget::from(claims);
            target.link_name = Some(invocation.target.link_name.clone());
            target
        };

        let resp = self
            .policy_manager
            .evaluate_action(Some(source), target, PolicyAction::PerformInvocation)
            .await?;
        if !resp.permitted {
            bail!(
                "Policy denied request to invoke actor `{}`: `{:?}`",
                resp.request_id,
                resp.message
            );
        };

        let maybe_resp = self
            .handle_invocation(
                &invocation.origin.contract_id,
                &invocation.operation,
                inv_msg,
            )
            .await
            .context("failed to handle invocation")?;

        match maybe_resp {
            Ok(resp_msg) => {
                let content_length = resp_msg.len();
                let resp_msg = if content_length > CHUNK_THRESHOLD_BYTES {
                    debug!(inv_id = invocation.id, "chunking invocation response");
                    self.chunk_endpoint
                        .chunkify_response(&invocation.id, Cursor::new(resp_msg))
                        .await
                        .context("failed to chunk invocation response")?;
                    vec![]
                } else {
                    resp_msg
                };
                Ok((
                    resp_msg,
                    content_length
                        .try_into()
                        .context("failed to convert content_length to u64")?,
                ))
            }
            Err(e) => Err(anyhow!(e)),
        }
    }

    #[instrument(skip_all)]
    async fn handle_message(&self, message: async_nats::Message) {
        let async_nats::Message {
            ref subject,
            ref reply,
            ref payload,
            ..
        } = message;

        match rmp_serde::from_slice::<Invocation>(payload) {
            Ok(invocation) => {
                if !invocation.trace_context.is_empty() {
                    wasmcloud_tracing::context::attach_span_context(&invocation.trace_context);
                } else if message.headers.is_some() {
                    // TODO: remove once all providers are built off the new SDK, which passes the trace context in the invocation
                    // fall back on message headers
                    opentelemetry_nats::attach_span_context(&message);
                }

                let invocation_id = invocation.id.clone();
                let origin = invocation.origin.clone();
                let target = invocation.target.clone();
                let operation = invocation.operation.clone();

                let res = self.handle_call(invocation).await;
                let injector = TraceContextInjector::default_with_span();
                let headers = injector_to_headers(&injector);
                let trace_context = injector.into();
                let inv_resp = match res {
                    Ok((msg, content_length)) => InvocationResponse {
                        msg,
                        invocation_id,
                        content_length,
                        trace_context,
                        ..Default::default()
                    },
                    Err(e) => {
                        error!(
                            ?origin,
                            ?target,
                            ?operation,
                            ?invocation_id,
                            ?e,
                            "failed to handle request"
                        );
                        InvocationResponse {
                            invocation_id,
                            error: Some(e.to_string()),
                            trace_context,
                            ..Default::default()
                        }
                    }
                };

                if let Some(reply) = reply {
                    match rmp_serde::to_vec_named(&inv_resp) {
                        Ok(buf) => {
                            if let Err(e) = self
                                .nats
                                .publish_with_headers(reply.clone(), headers, buf.into())
                                .await
                            {
                                error!(?reply, ?e, "failed to publish response to request");
                            }
                        }
                        Err(e) => {
                            error!(?e, "failed to encode response");
                        }
                    }
                }
            }
            Err(e) => {
                error!(?subject, ?e, "failed to decode invocation"); // Note: this won't be traced
            }
        }
    }
}

type Annotations = BTreeMap<String, String>;

#[derive(Debug)]
struct Actor {
    pool: ActorInstancePool,
    instances: RwLock<HashMap<Annotations, Vec<Arc<ActorInstance>>>>,
    image_ref: String,
    handler: Handler,
}

#[derive(Debug)]
struct ProviderInstance {
    child: JoinHandle<()>,
    id: Ulid,
    annotations: Annotations,
}

#[derive(Debug)]
struct Provider {
    claims: jwt::Claims<jwt::CapabilityProvider>,
    instances: HashMap<String, ProviderInstance>,
    image_ref: String,
}

/// wasmCloud Host
pub struct Host {
    // TODO: Clean up actors after stop
    actors: RwLock<HashMap<String, Arc<Actor>>>,
    chunk_endpoint: ChunkEndpoint,
    cluster_key: Arc<KeyPair>,
    cluster_issuers: Vec<String>,
    event_builder: EventBuilderV10,
    friendly_name: String,
    heartbeat: AbortHandle,
    host_config: HostConfig,
    host_key: Arc<KeyPair>,
    labels: HashMap<String, String>,
    ctl_topic_prefix: String,
    /// NATS client to use for control interface subscriptions and jetstream queries
    ctl_nats: async_nats::Client,
    /// NATS client to use for actor RPC calls
    rpc_nats: async_nats::Client,
    /// NATS client to use for communicating with capability providers
    prov_rpc_nats: async_nats::Client,
    data: async_nats::jetstream::kv::Store,
    data_watch: AbortHandle,
    policy_manager: Arc<PolicyManager>,
    providers: RwLock<HashMap<String, Provider>>,
    registry_config: RwLock<HashMap<String, RegistryConfig>>,
    runtime: Runtime,
    start_at: Instant,
    stop_tx: watch::Sender<Option<Instant>>,
    stop_rx: watch::Receiver<Option<Instant>>,
    queue: AbortHandle,
    aliases: Arc<RwLock<HashMap<String, WasmCloudEntity>>>,
    links: RwLock<HashMap<String, LinkDefinition>>,
    actor_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::Actor>>>>, // TODO: use a single map once Claims is an enum
    provider_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::CapabilityProvider>>>>,
}

#[allow(clippy::large_enum_variant)] // Without this clippy complains actor is at least 0 bytes while provider is at least 280 bytes. That doesn't make sense
enum Claims {
    Actor(jwt::Claims<jwt::Actor>),
    Provider(jwt::Claims<jwt::CapabilityProvider>),
}

impl Claims {
    fn subject(&self) -> &str {
        match self {
            Claims::Actor(claims) => &claims.subject,
            Claims::Provider(claims) => &claims.subject,
        }
    }
}

impl From<StoredClaims> for Claims {
    fn from(claims: StoredClaims) -> Self {
        let name = (!claims.name.is_empty()).then_some(claims.name);
        let rev = claims.revision.parse().ok();
        let ver = (!claims.version.is_empty()).then_some(claims.version);

        // rely on the fact that serialized actor claims don't include a contract_id
        if claims.contract_id.is_empty() {
            let tags =
                (!claims.tags.is_empty()).then(|| claims.tags.split(',').map(Into::into).collect());
            let caps = (!claims.capabilities.is_empty())
                .then(|| claims.capabilities.split(',').map(Into::into).collect());
            let call_alias = (!claims.call_alias.is_empty()).then_some(claims.call_alias);
            let metadata = jwt::Actor {
                name,
                tags,
                caps,
                rev,
                ver,
                call_alias,
                ..Default::default()
            };
            let claims = ClaimsBuilder::new()
                .subject(&claims.subject)
                .issuer(&claims.issuer)
                .with_metadata(metadata)
                .build();
            Claims::Actor(claims)
        } else {
            let config_schema: Option<serde_json::Value> = claims
                .config_schema
                .and_then(|schema| serde_json::from_str(&schema).ok());
            let metadata = jwt::CapabilityProvider {
                name,
                capid: claims.contract_id,
                rev,
                ver,
                config_schema,
                ..Default::default()
            };
            let claims = ClaimsBuilder::new()
                .subject(&claims.subject)
                .issuer(&claims.issuer)
                .with_metadata(metadata)
                .build();
            Claims::Provider(claims)
        }
    }
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
    jetstream: &async_nats::jetstream::Context,
    bucket: &str,
) -> anyhow::Result<()> {
    // Don't create the bucket if it already exists
    if let Ok(_store) = jetstream.get_key_value(bucket).await {
        info!("lattice metadata bucket {bucket} already exists. Skipping creation.");
        return Ok(());
    }

    match jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
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

/// Given the NATS address, authentication jwt, seed, tls requirement and optional request timeout,
/// attempt to establish connection.
///
///
/// # Errors
///
/// Returns an error if:
/// - Only one of JWT or seed is specified, as we cannot authenticate with only one of them
/// - Connection fails
async fn connect_nats(
    addr: impl async_nats::ToServerAddrs,
    jwt: Option<&String>,
    key: Option<Arc<KeyPair>>,
    require_tls: bool,
    request_timeout: Option<Duration>,
) -> anyhow::Result<async_nats::Client> {
    let opts = async_nats::ConnectOptions::new().require_tls(require_tls);
    let opts = match (jwt, key) {
        (Some(jwt), Some(key)) => opts.jwt(jwt.to_string(), {
            move |nonce| {
                let key = key.clone();
                async move { key.sign(&nonce).map_err(async_nats::AuthError::new) }
            }
        }),
        (Some(_), None) | (None, Some(_)) => {
            bail!("cannot authenticate if only one of jwt or seed is specified")
        }
        _ => opts,
    };
    let opts = if let Some(timeout) = request_timeout {
        opts.request_timeout(Some(timeout))
    } else {
        opts
    };
    opts.connect(addr)
        .await
        .context("failed to connect to NATS")
}

#[derive(Debug, Default)]
struct SupplementalConfig {
    registry_config: Option<HashMap<String, RegistryConfig>>,
}

#[instrument(skip(ctl_nats))]
async fn load_supplemental_config(
    ctl_nats: &async_nats::Client,
    lattice_prefix: &str,
    labels: &HashMap<String, String>,
) -> anyhow::Result<SupplementalConfig> {
    #[derive(Deserialize, Default)]
    struct SerializedSupplementalConfig {
        #[serde(default, rename = "registryCredentials")]
        registry_credentials: Option<HashMap<String, RegistryCredential>>,
    }

    let cfg_topic = format!("wasmbus.cfg.{lattice_prefix}.req");
    let cfg_payload = serde_json::to_vec(&json!({
        "labels": labels,
    }))
    .context("failed to serialize config payload")?;

    match ctl_nats.request(cfg_topic, cfg_payload.into()).await {
        Ok(resp) => {
            match serde_json::from_slice::<SerializedSupplementalConfig>(resp.payload.as_ref()) {
                Ok(ser_cfg) => Ok(SupplementalConfig {
                    registry_config: ser_cfg
                        .registry_credentials
                        .map(|creds| creds.into_iter().map(|(k, v)| (k, v.into())).collect()),
                }),
                Err(e) => {
                    error!(
                        ?e,
                        "failed to deserialize supplemental config. Defaulting to empty config"
                    );
                    Ok(SupplementalConfig::default())
                }
            }
        }
        Err(e) => {
            error!(
                ?e,
                "failed to request supplemental config. Defaulting to empty config"
            );
            Ok(SupplementalConfig::default())
        }
    }
}

#[instrument(skip_all)]
async fn merge_registry_config(
    registry_config: &RwLock<HashMap<String, RegistryConfig>>,
    oci_opts: OciConfig,
) -> () {
    let mut registry_config = registry_config.write().await;
    let allow_latest = oci_opts.allow_latest;

    // update auth for specific registry, if provided
    if let Some(reg) = oci_opts.oci_registry {
        match registry_config.entry(reg) {
            Entry::Occupied(_entry) => {
                // note we don't update config here, since the config service should take priority
            }
            Entry::Vacant(entry) => {
                entry.insert(RegistryConfig {
                    reg_type: RegistryType::Oci,
                    auth: RegistryAuth::from((oci_opts.oci_user, oci_opts.oci_password)),
                    ..Default::default()
                });
            }
        }
    }

    // update or create entry for all registries in allowed_insecure
    oci_opts
        .allowed_insecure
        .into_iter()
        .for_each(|reg| match registry_config.entry(reg) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().allow_insecure = true;
            }
            Entry::Vacant(entry) => {
                entry.insert(RegistryConfig {
                    reg_type: RegistryType::Oci,
                    allow_insecure: true,
                    ..Default::default()
                });
            }
        });

    // set allow_latest for all registries
    registry_config
        .values_mut()
        .for_each(|config| config.allow_latest = allow_latest);
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
    wave water resonance sun timber dream cherry tree fog autocorrect
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
    #[instrument(skip_all)]
    pub async fn new(
        config: HostConfig,
    ) -> anyhow::Result<(Arc<Self>, impl Future<Output = anyhow::Result<()>>)> {
        let cluster_key = if let Some(cluster_key) = &config.cluster_key {
            ensure!(cluster_key.key_pair_type() == KeyPairType::Cluster);
            Arc::clone(cluster_key)
        } else {
            Arc::new(KeyPair::new(KeyPairType::Cluster))
        };
        let mut cluster_issuers = config.cluster_issuers.clone().unwrap_or_default();
        let cluster_pub_key = cluster_key.public_key();
        if !cluster_issuers.contains(&cluster_pub_key) {
            debug!(cluster_pub_key, "adding cluster key to cluster issuers");
            cluster_issuers.push(cluster_pub_key);
        }
        let host_key = if let Some(host_key) = &config.host_key {
            ensure!(host_key.key_pair_type() == KeyPairType::Server);
            Arc::clone(host_key)
        } else {
            Arc::new(KeyPair::new(KeyPairType::Server))
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

        let ((ctl_nats, queue), rpc_nats, prov_rpc_nats) = try_join!(
            async {
                debug!(
                    ctl_nats_url = config.ctl_nats_url.as_str(),
                    "connecting to NATS control server"
                );
                let ctl_nats = connect_nats(
                    config.ctl_nats_url.as_str(),
                    config.ctl_jwt.as_ref(),
                    config.ctl_key.clone(),
                    config.ctl_tls,
                    None,
                )
                .await
                .context("failed to establish NATS control server connection")?;
                let queue = Queue::new(
                    &ctl_nats,
                    &config.ctl_topic_prefix,
                    &config.lattice_prefix,
                    &cluster_key,
                    &host_key,
                )
                .await
                .context("failed to initialize queue")?;
                ctl_nats.flush().await.context("failed to flush")?;
                Ok((ctl_nats, queue))
            },
            async {
                debug!(
                    rpc_nats_url = config.rpc_nats_url.as_str(),
                    "connecting to NATS RPC server"
                );
                connect_nats(
                    config.rpc_nats_url.as_str(),
                    config.rpc_jwt.as_ref(),
                    config.rpc_key.clone(),
                    config.rpc_tls,
                    Some(config.rpc_timeout),
                )
                .await
                .context("failed to establish NATS RPC server connection")
            },
            async {
                debug!(
                    prov_rpc_nats_url = config.prov_rpc_nats_url.as_str(),
                    "connecting to NATS Provider RPC server"
                );
                connect_nats(
                    config.prov_rpc_nats_url.as_str(),
                    config.prov_rpc_jwt.as_ref(),
                    config.prov_rpc_key.clone(),
                    config.prov_rpc_tls,
                    None,
                )
                .await
                .context("failed to establish NATS provider RPC server connection")
            }
        )?;

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

        let ctl_jetstream = if let Some(domain) = config.js_domain.as_ref() {
            async_nats::jetstream::with_domain(ctl_nats.clone(), domain)
        } else {
            async_nats::jetstream::new(ctl_nats.clone())
        };
        let bucket = format!("LATTICEDATA_{}", config.lattice_prefix);
        create_lattice_metadata_bucket(&ctl_jetstream, &bucket).await?;

        let data = ctl_jetstream
            .get_key_value(&bucket)
            .await
            .map_err(|e| anyhow!(e).context("failed to acquire data bucket"))?;

        let chunk_endpoint = ChunkEndpoint::with_client(
            &config.lattice_prefix,
            rpc_nats.clone(),
            config.js_domain.as_ref(),
        );

        let (queue_abort, queue_abort_reg) = AbortHandle::new_pair();
        let (heartbeat_abort, heartbeat_abort_reg) = AbortHandle::new_pair();
        let (data_watch_abort, data_watch_abort_reg) = AbortHandle::new_pair();

        let supplemental_config = if config.config_service_enabled {
            load_supplemental_config(&ctl_nats, &config.lattice_prefix, &labels).await?
        } else {
            SupplementalConfig::default()
        };

        let registry_config = RwLock::new(supplemental_config.registry_config.unwrap_or_default());
        merge_registry_config(&registry_config, config.oci_opts.clone()).await;

        let policy_manager = PolicyManager::new(
            ctl_nats.clone(),
            PolicyHostInfo {
                public_key: host_key.public_key(),
                lattice_id: config.lattice_prefix.clone(),
                labels: labels.clone(),
                cluster_issuers: cluster_issuers.clone(),
            },
            config.policy_service_config.policy_topic.clone(),
            config.policy_service_config.policy_timeout_ms,
            config.policy_service_config.policy_changes_topic.clone(),
        )
        .await?;

        let host = Host {
            actors: RwLock::default(),
            chunk_endpoint,
            cluster_key,
            cluster_issuers,
            event_builder,
            friendly_name,
            heartbeat: heartbeat_abort.clone(),
            ctl_topic_prefix: config.ctl_topic_prefix.clone(),
            host_key,
            labels,
            ctl_nats,
            rpc_nats,
            prov_rpc_nats,
            host_config: config,
            data: data.clone(),
            data_watch: data_watch_abort.clone(),
            policy_manager,
            providers: RwLock::default(),
            registry_config,
            runtime,
            start_at,
            stop_rx,
            stop_tx,
            queue: queue_abort.clone(),
            aliases: Arc::default(),
            links: RwLock::default(),
            actor_claims: Arc::default(),
            provider_claims: Arc::default(),
        };

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

        host.publish_event("host_started", start_evt)
            .await
            .context("failed to publish start event")?;
        info!(
            host_id = host.host_key.public_key(),
            "wasmCloud host started"
        );

        Ok((Arc::clone(&host), async move {
            heartbeat_abort.abort();
            queue_abort.abort();
            data_watch_abort.abort();
            host.policy_manager.policy_changes.abort();
            let _ = try_join!(queue, data_watch, heartbeat).context("failed to await tasks")?;
            host.publish_event(
                "host_stopped",
                json!({
                    "labels": host.labels,
                }),
            )
            .await
            .context("failed to publish stop event")?;
            // Before we exit, make sure to flush all messages or we may lose some that we've
            // thought were sent (like the host_stopped event)
            host.ctl_nats
                .flush()
                .await
                .context("failed to flush ctl client")?;
            host.rpc_nats
                .flush()
                .await
                .context("failed to flush rpc client")?;
            host.prov_rpc_nats
                .flush()
                .await
                .context("failed to flush prov rpc client")
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
            "uptime_human": human_friendly_uptime(uptime),
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
        event::publish(
            &self.event_builder,
            &self.ctl_nats,
            &self.host_config.lattice_prefix,
            name,
            data,
        )
        .await
    }

    /// Instantiate an actor and publish the actor start events.
    #[allow(clippy::too_many_arguments)] // TODO: refactor into a config struct
    #[instrument(skip(self, claims, annotations, host_id, actor_ref, pool, handler))]
    async fn instantiate_actor(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        annotations: &Annotations,
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
            let claims = claims.clone();
            async move {
                let calls = self
                    .rpc_nats
                    .queue_subscribe(topic.clone(), topic)
                    .await
                    .context("failed to subscribe to actor call queue")?;

                let (calls_abort, calls_abort_reg) = AbortHandle::new_pair();
                let id = Ulid::new();
                let instance = Arc::new(ActorInstance {
                    nats: self.rpc_nats.clone(),
                    pool,
                    id,
                    calls: calls_abort,
                    runtime: self.runtime.clone(),
                    handler: handler.clone(),
                    chunk_endpoint: self.chunk_endpoint.clone(),
                    valid_issuers: self.cluster_issuers.clone(),
                    policy_manager: Arc::clone(&self.policy_manager),
                    actor_claims: Arc::clone(&self.actor_claims),
                    provider_claims: Arc::clone(&self.provider_claims),
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
                        &claims,
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
    #[instrument(skip(self, instances))]
    async fn uninstantiate_actor(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        annotations: &Annotations,
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

    #[instrument(skip(self, entry, actor, annotations))]
    async fn start_actor<'a>(
        &self,
        entry: hash_map::VacantEntry<'a, String, Arc<Actor>>,
        actor: wasmcloud_runtime::Actor,
        actor_ref: String,
        count: NonZeroUsize,
        host_id: &str,
        annotations: impl Into<Annotations>,
    ) -> anyhow::Result<&'a mut Arc<Actor>> {
        trace!(actor_ref, "starting new actor");

        let annotations = annotations.into();
        let claims = actor.claims().context("claims missing")?;
        self.store_claims(Claims::Actor(claims.clone()))
            .await
            .context("failed to store claims")?;

        let links = self.links.read().await;
        let links = links
            .values()
            .filter(|ld| ld.actor_id == claims.subject)
            .fold(
                HashMap::<_, HashMap<_, _>>::default(),
                |mut links,
                 LinkDefinition {
                     link_name,
                     contract_id,
                     provider_id,
                     ..
                 }| {
                    links.entry(contract_id.clone()).or_default().insert(
                        link_name.clone(),
                        WasmCloudEntity {
                            link_name: link_name.clone(),
                            contract_id: contract_id.clone(),
                            public_key: provider_id.clone(),
                        },
                    );
                    links
                },
            );
        let origin = WasmCloudEntity {
            public_key: claims.subject.clone(),
            ..Default::default()
        };
        let handler = Handler {
            nats: self.rpc_nats.clone(),
            lattice_prefix: self.host_config.lattice_prefix.clone(),
            origin,
            cluster_key: Arc::clone(&self.cluster_key),
            claims: claims.clone(),
            aliases: Arc::clone(&self.aliases),
            links: Arc::new(RwLock::new(links)),
            targets: Arc::new(RwLock::default()),
            host_key: Arc::clone(&self.host_key),
            chunk_endpoint: self.chunk_endpoint.clone(),
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

    #[instrument(skip(self, entry))]
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
        let registry_config = self.registry_config.read().await;
        let actor = fetch_actor(
            actor_ref,
            self.host_config.allow_file_load,
            &registry_config,
        )
        .await
        .context("failed to fetch actor")?;
        let actor = wasmcloud_runtime::Actor::new(&self.runtime, actor)
            .context("failed to initialize actor")?;
        Ok(actor)
    }

    #[instrument(skip(self))]
    async fn store_actor_claims(&self, claims: jwt::Claims<jwt::Actor>) -> anyhow::Result<()> {
        if let Some(call_alias) = claims
            .metadata
            .as_ref()
            .and_then(|jwt::Actor { call_alias, .. }| call_alias.clone())
        {
            let mut aliases = self.aliases.write().await;
            match aliases.entry(call_alias) {
                Entry::Occupied(entry) => {
                    ensure!(
                        entry.get().public_key == claims.subject,
                        "call alias `{}` clash between `{}` and `{}`",
                        entry.key(),
                        entry.get().public_key,
                        claims.subject
                    );
                }
                Entry::Vacant(entry) => {
                    entry.insert(WasmCloudEntity {
                        public_key: claims.subject.clone(),
                        ..Default::default()
                    });
                }
            }
        }
        let mut actor_claims = self.actor_claims.write().await;
        actor_claims.insert(claims.subject.clone(), claims);
        Ok(())
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

        let actor = self.fetch_actor(&actor_ref).await?;
        let claims = actor.claims().context("claims missing")?;
        let actor_id = claims.subject.clone();
        let resp = self
            .policy_manager
            .evaluate_action(
                None,
                PolicyRequestTarget::from(claims.clone()),
                PolicyAction::StartActor,
            )
            .await?;
        if !resp.permitted {
            bail!(
                "Policy denied request to start actor `{}`: `{:?}`",
                resp.request_id,
                resp.message
            )
        };

        let annotations = annotations.unwrap_or_default().into_iter().collect();
        match (
            self.actors.write().await.entry(actor_id),
            NonZeroUsize::new(count.into()),
        ) {
            (hash_map::Entry::Vacant(_), None) => {}
            (hash_map::Entry::Vacant(entry), Some(count)) => {
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
        annotations: HashMap<String, String>,
        count: u16,
        host_id: &str,
    ) -> anyhow::Result<()> {
        debug!("launch actor");

        let actor = self.fetch_actor(&actor_ref).await?;
        let claims = actor.claims().context("claims missing")?;
        let actor_id = claims.subject.clone();
        let resp = self
            .policy_manager
            .evaluate_action(
                None,
                PolicyRequestTarget::from(claims.clone()),
                PolicyAction::StartActor,
            )
            .await?;
        if !resp.permitted {
            bail!(
                "Policy denied request to start actor `{}`: `{:?}`",
                resp.request_id,
                resp.message
            )
        };

        let annotations = annotations.into_iter().collect();
        let Some(count) = NonZeroUsize::new(count.into()) else {
            // NOTE: This mimics OTP behavior
            self.publish_event(
                "actors_started",
                event::actors_started(claims, &annotations, host_id, 0usize, actor_ref),
            )
            .await?;
            return Ok(())
        };

        match self.actors.write().await.entry(actor_id) {
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
                .handle_launch_actor_task(
                    actor_ref.clone(),
                    annotations.unwrap_or_default(),
                    count,
                    &host_id,
                )
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

        let annotations: Annotations = annotations.unwrap_or_default().into_iter().collect();
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
        let annotations = annotations.unwrap_or_default().into_iter().collect(); // convert from HashMap to BTreeMap
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
        self.store_claims(Claims::Actor(new_claims.clone()))
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
        annotations: HashMap<String, String>,
        host_id: &str,
    ) -> anyhow::Result<()> {
        debug!("launch provider");

        let registry_config = self.registry_config.read().await;
        let (path, claims) = crate::fetch_provider(
            provider_ref,
            link_name,
            self.host_config.allow_file_load,
            &registry_config,
        )
        .await
        .context("failed to fetch provider")?;

        let mut target = PolicyRequestTarget::from(claims.clone());
        target.link_name = Some(link_name.to_owned());
        let PolicyResponse {
            permitted,
            request_id,
            message,
        } = self
            .policy_manager
            .evaluate_action(None, target, PolicyAction::StartProvider)
            .await?;
        ensure!(
            permitted,
            "policy denied request to start provider `{request_id}`: `{message:?}`",
        );
        self.store_claims(Claims::Provider(claims.clone()))
            .await
            .context("failed to store claims")?;

        let annotations: Annotations = annotations.into_iter().collect();
        let mut providers = self.providers.write().await;
        let Provider { instances, .. } =
            providers.entry(claims.subject.clone()).or_insert(Provider {
                claims: claims.clone(),
                image_ref: provider_ref.into(),
                instances: HashMap::default(),
            });
        if let hash_map::Entry::Vacant(entry) = instances.entry(link_name.into()) {
            let id = Ulid::new();
            let invocation_seed = self
                .cluster_key
                .seed()
                .context("cluster key seed missing")?;
            let links = self.links.read().await;
            // TODO: update type of links to use wasmcloud_core::LinkDefinition
            let link_definitions: Vec<_> = links
                .clone()
                .into_values()
                .filter(|ld| ld.provider_id == claims.subject && ld.link_name == link_name)
                .map(|ld| wasmcloud_core::LinkDefinition {
                    actor_id: ld.actor_id,
                    provider_id: ld.provider_id,
                    link_name: ld.link_name,
                    contract_id: ld.contract_id,
                    values: ld.values.into_iter().collect(),
                })
                .collect();
            let lattice_rpc_user_seed = self
                .host_config
                .prov_rpc_key
                .as_ref()
                .map(|key| key.seed())
                .transpose()
                .context("private key missing for provider RPC key")?;
            let default_rpc_timeout_ms = Some(
                self.host_config
                    .rpc_timeout
                    .as_millis()
                    .try_into()
                    .context("failed to convert rpc_timeout to u64")?,
            );
            let otel_config = OtelConfig {
                traces_exporter: self.host_config.otel_config.traces_exporter.clone(),
                exporter_otlp_endpoint: self.host_config.otel_config.exporter_otlp_endpoint.clone(),
            };
            let host_data = HostData {
                host_id: self.host_key.public_key(),
                lattice_rpc_prefix: self.host_config.lattice_prefix.clone(),
                link_name: link_name.to_string(),
                lattice_rpc_user_jwt: self.host_config.prov_rpc_jwt.clone().unwrap_or_default(),
                lattice_rpc_user_seed: lattice_rpc_user_seed.unwrap_or_default(),
                lattice_rpc_url: self.host_config.prov_rpc_nats_url.to_string(),
                env_values: vec![],
                instance_id: Uuid::from_u128(id.into()).to_string(),
                provider_key: claims.subject.clone(),
                link_definitions,
                config_json: configuration,
                default_rpc_timeout_ms,
                cluster_issuers: self.cluster_issuers.clone(),
                invocation_seed,
                log_level: Some(self.host_config.log_level.clone()),
                structured_logging: self.host_config.enable_structured_logging,
                otel_config,
            };
            let host_data =
                serde_json::to_vec(&host_data).context("failed to serialize provider data")?;

            debug!(
                ?path,
                host_data = &*String::from_utf8_lossy(&host_data),
                "spawn provider process"
            );
            let mut child = process::Command::new(&path)
                .env_clear()
                // TODO: remove these once all providers are updated to use the new SDK
                .env(
                    "OTEL_TRACES_EXPORTER",
                    self.host_config
                        .otel_config
                        .traces_exporter
                        .clone()
                        .unwrap_or_default(),
                )
                .env(
                    "OTEL_EXPORTER_OTLP_ENDPOINT",
                    self.host_config
                        .otel_config
                        .exporter_otlp_endpoint
                        .clone()
                        .unwrap_or_default(),
                )
                .stdin(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("failed to spawn provider process")?;
            let mut stdin = child.stdin.take().context("failed to take stdin")?;
            stdin
                .write_all(STANDARD.encode(&host_data).as_bytes())
                .await
                .context("failed to write provider data")?;
            stdin
                .write_all(b"\r\n")
                .await
                .context("failed to write newline")?;
            stdin.shutdown().await.context("failed to close stdin")?;

            // TODO: Change method receiver to Arc<Self> and `move` into the closure
            let prov_nats = self.prov_rpc_nats.clone();
            let ctl_nats = self.ctl_nats.clone();
            let event_builder = self.event_builder.clone();
            // NOTE: health_ prefix here is to allow us to move the variables into the closure
            let health_lattice_prefix = self.host_config.lattice_prefix.clone();
            let health_provider_id = claims.subject.to_string();
            let health_link_name = link_name.to_string();
            let health_contract_id = claims.metadata.clone().map(|m| m.capid).unwrap_or_default();
            let child = spawn(async move {
                // Check the health of the provider every 30 seconds
                let mut health_check = tokio::time::interval(Duration::from_secs(30));
                let mut previous_healthy = false;
                // Allow the provider 5 seconds to initialize
                health_check.reset_after(Duration::from_secs(5));
                let health_topic =
                    format!("wasmbus.rpc.{health_lattice_prefix}.{health_provider_id}.{health_link_name}.health");
                // TODO: Refactor this logic to simplify nesting
                loop {
                    select! {
                        _ = health_check.tick() => {
                            trace!(provider_id=health_provider_id, "performing provider health check");
                            let request = async_nats::Request::new()
                                .payload(Bytes::new())
                                .headers(injector_to_headers(&TraceContextInjector::default_with_span()));
                            if let Ok(async_nats::Message { payload, ..}) = prov_nats.send_request(
                                health_topic.clone(),
                                request,
                                ).await {
                                    match (rmp_serde::from_slice::<HealthCheckResponse>(&payload), previous_healthy) {
                                        (Ok(HealthCheckResponse { healthy: true, ..}), false) => {
                                            trace!(provider_id=health_provider_id, "provider health check succeeded");
                                            previous_healthy = true;
                                            if let Err(e) = event::publish(
                                                &event_builder,
                                                &ctl_nats,
                                                &health_lattice_prefix,
                                                "health_check_passed",
                                                event::provider_health_check(
                                                    &health_provider_id,
                                                    &health_link_name,
                                                    &health_contract_id,
                                                )
                                            ).await {
                                                warn!(?e, "failed to publish provider health check succeeded event");
                                            }
                                        },
                                        (Ok(HealthCheckResponse { healthy: false, ..}), true) => {
                                            trace!(provider_id=health_provider_id, "provider health check failed");
                                            previous_healthy = false;
                                            if let Err(e) = event::publish(
                                                &event_builder,
                                                &ctl_nats,
                                                &health_lattice_prefix,
                                                "health_check_failed",
                                                event::provider_health_check(
                                                    &health_provider_id,
                                                    &health_link_name,
                                                    &health_contract_id,
                                                )
                                            ).await {
                                                warn!(?e, "failed to publish provider health check failed event");
                                            }
                                        }
                                        // If the provider health status didn't change, we don't need to publish an event
                                        (Ok(_), _) => (),
                                        _ => warn!("failed to deserialize provider health check response"),
                                    }
                                }
                                else {
                                    warn!("failed to request provider health, retrying in 30 seconds");
                                }
                        }
                        exit_status = child.wait() => match exit_status {
                            Ok(status) => {
                                debug!("`{}` exited with `{status:?}`", path.display());
                                break;
                            }
                            Err(e) => {
                                warn!("failed to wait for `{}` to execute: {e}", path.display());
                                break;
                            }
                        }
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
                    annotations.unwrap_or_default(),
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

        let mut providers = self.providers.write().await;
        let hash_map::Entry::Occupied(mut entry) = providers.entry(provider_ref.clone()) else {
            return Ok(SUCCESS.into());
        };
        let provider = entry.get_mut();
        let instances = &mut provider.instances;
        if let hash_map::Entry::Occupied(entry) = instances.entry(link_name.clone()) {
            let ProviderInstance {
                id,
                child,
                annotations,
                ..
            } = entry.remove();

            // Send a request to the provider, requesting a graceful shutdown
            let req = serde_json::to_vec(&json!({ "host_id": host_id }))
                .context("failed to encode provider stop request")?;
            let req = async_nats::Request::new()
                .payload(req.into())
                .timeout(self.host_config.provider_shutdown_delay)
                .headers(injector_to_headers(
                    &TraceContextInjector::default_with_span(),
                ));
            if let Err(e) = self
                .prov_rpc_nats
                .send_request(
                    format!(
                        "wasmbus.rpc.{}.{provider_ref}.{link_name}.shutdown",
                        self.host_config.lattice_prefix
                    ),
                    req,
                )
                .await
            {
                warn!(
                    ?e,
                    "provider did not gracefully shut down in time, shutting down forcefully"
                );
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
                            let annotations = Some(annotations.clone().into_iter().collect());
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
                            let annotations = Some(annotations.clone().into_iter().collect());
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

        let (actor_claims, provider_claims) =
            join!(self.actor_claims.read(), self.provider_claims.read());
        let actor_claims = actor_claims.values().cloned().map(Claims::Actor);
        let provider_claims = provider_claims.values().cloned().map(Claims::Provider);
        let claims = actor_claims
            .chain(provider_claims)
            .flat_map(TryFrom::try_from)
            .collect();
        let res = serde_json::to_vec(&ClaimsResponse { claims })
            .context("failed to serialize response")?;
        Ok(res.into())
    }

    #[instrument(skip(self))]
    async fn handle_links(&self) -> anyhow::Result<Bytes> {
        let links = self.links.read().await;
        let links = links.values().cloned().collect();
        let res = serde_json::to_vec(&LinkDefinitionList { links })
            .context("failed to serialize response")?;
        Ok(res.into())
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

    #[instrument(skip(self, payload))]
    async fn handle_registries_put(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        let registry_creds: RegistryCredentialMap = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize registries put command")?;

        debug!(
            registries = ?registry_creds.keys(),
            "updating registry config",
        );

        let mut registry_config = self.registry_config.write().await;
        for (reg, new_creds) in registry_creds {
            let mut new_config = RegistryConfig::from(new_creds);
            match registry_config.entry(reg) {
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().auth = new_config.auth;
                }
                hash_map::Entry::Vacant(entry) => {
                    new_config.allow_latest = self.host_config.oci_opts.allow_latest;
                    entry.insert(new_config);
                }
            }
        }

        Ok(SUCCESS.into())
    }

    #[instrument(skip(self, _payload))]
    async fn handle_ping_hosts(&self, _payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        let uptime = self.start_at.elapsed();
        let cluster_issuers = self.cluster_issuers.clone().join(",");

        let buf = serde_json::to_vec(&json!({
          "id": self.host_key.public_key(),
          "issuer": self.cluster_key.public_key(),
          "labels": self.labels,
          "friendly_name": self.friendly_name,
          "uptime_seconds": uptime.as_secs(),
          "uptime_human": human_friendly_uptime(uptime),
          "version": env!("CARGO_PKG_VERSION"),
          "cluster_issuers": cluster_issuers,
          "js_domain": self.host_config.js_domain,
          "ctl_host": self.host_config.ctl_nats_url.to_string(),
          "prov_rpc_host": self.host_config.prov_rpc_nats_url.to_string(),
          "rpc_host": self.host_config.rpc_nats_url.to_string(),
          "lattice_prefix": self.host_config.lattice_prefix,
        }))
        .context("failed to encode reply")?;
        Ok(buf.into())
    }

    #[instrument(skip_all)]
    async fn handle_message(self: Arc<Self>, message: async_nats::Message) {
        let async_nats::Message {
            ref subject,
            ref reply,
            ref payload,
            ..
        } = message;

        opentelemetry_nats::attach_span_context(&message);
        // Skip the topic prefix and then the lattice prefix
        // e.g. `wasmbus.ctl.{prefix}`
        let mut parts = subject
            .trim()
            .trim_start_matches(&self.ctl_topic_prefix)
            .trim_start_matches('.')
            .split('.')
            .skip(1);
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
        let headers = injector_to_headers(&TraceContextInjector::default_with_span());
        match (reply, res) {
            (Some(reply), Ok(Some(buf))) => {
                if let Err(e) = self
                    .ctl_nats
                    .publish_with_headers(reply.clone(), headers, buf)
                    .await
                {
                    error!("failed to publish success in response to `{subject}` request: {e:?}");
                }
            }
            (Some(reply), Err(e)) => {
                if let Err(e) = self
                    .ctl_nats
                    .publish_with_headers(
                        reply.clone(),
                        headers,
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

    #[instrument(skip_all)]
    async fn store_claims(&self, claims: Claims) -> anyhow::Result<()> {
        match &claims {
            Claims::Actor(claims) => {
                self.store_actor_claims(claims.clone()).await?;
            }
            Claims::Provider(claims) => {
                let mut provider_claims = self.provider_claims.write().await;
                provider_claims.insert(claims.subject.clone(), claims.clone());
            }
        };
        let claims: StoredClaims = claims.try_into()?;
        let key = format!("CLAIMS_{}", claims.subject);
        trace!(?claims, ?key, "storing claims");

        let bytes = serde_json::to_vec(&claims)
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
        if let Some(actor) = self.actors.read().await.get(actor_id) {
            let mut links = actor.handler.links.write().await;
            links.entry(contract_id.clone()).or_default().insert(
                ld.link_name.clone(),
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

        let msgp = rmp_serde::to_vec_named(ld).context("failed to encode link definition")?;
        let lattice_prefix = &self.host_config.lattice_prefix;
        self.prov_rpc_nats
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice_prefix}.{provider_id}.{link_name}.linkdefs.put",),
                injector_to_headers(&TraceContextInjector::default_with_span()),
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
        if let Some(actor) = self.actors.read().await.get(actor_id) {
            let mut links = actor.handler.links.write().await;
            if let Some(links) = links.get_mut(contract_id) {
                links.remove(link_name);
            }
        }

        self.publish_event(
            "linkdef_deleted",
            event::linkdef_deleted(id, actor_id, provider_id, link_name, contract_id, values),
        )
        .await?;

        let msgp = rmp_serde::to_vec_named(ld).context("failed to encode link definition")?;
        let lattice_prefix = &self.host_config.lattice_prefix;
        self.prov_rpc_nats
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice_prefix}.{provider_id}.{link_name}.linkdefs.del",),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                msgp.into(),
            )
            .await
            .context("failed to publish link definition deletion")?;
        Ok(())
    }

    #[instrument(skip(self, pubkey, value))]
    async fn process_claims_put(
        &self,
        pubkey: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> anyhow::Result<()> {
        let pubkey = pubkey.as_ref();

        debug!(pubkey, "process claim entry put");

        let stored_claims: StoredClaims =
            serde_json::from_slice(value.as_ref()).context("failed to decode stored claims")?;
        let claims = Claims::from(stored_claims);

        ensure!(claims.subject() == pubkey, "subject mismatch");
        match claims {
            Claims::Actor(claims) => self.store_actor_claims(claims).await,
            Claims::Provider(claims) => {
                let mut provider_claims = self.provider_claims.write().await;
                provider_claims.insert(claims.subject.clone(), claims);
                Ok(())
            }
        }
    }

    #[instrument(skip(self, pubkey, value))]
    async fn process_claims_delete(
        &self,
        pubkey: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> anyhow::Result<()> {
        let pubkey = pubkey.as_ref();

        debug!(pubkey, "process claim entry deletion");

        let stored_claims: StoredClaims =
            serde_json::from_slice(value.as_ref()).context("failed to decode stored claims")?;
        let claims = Claims::from(stored_claims);

        ensure!(claims.subject() == pubkey, "subject mismatch");

        match claims {
            Claims::Actor(claims) => {
                let mut actor_claims = self.actor_claims.write().await;
                actor_claims.remove(&claims.subject);

                let Some(call_alias) = claims.metadata.and_then(|m| m.call_alias) else {
                    return Ok(());
                };

                let mut aliases = self.aliases.write().await;
                aliases
                    .remove(&call_alias)
                    .context("attempt to remove a non-existent call alias")?;
            }
            Claims::Provider(claims) => {
                let mut provider_claims = self.provider_claims.write().await;
                provider_claims.remove(&claims.subject);
            }
        }

        Ok(())
    }

    #[instrument(skip_all)]
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
            (Operation::Put, Some("CLAIMS"), Some(pubkey)) => {
                self.process_claims_put(pubkey, value).await
            }
            (Operation::Delete, Some("CLAIMS"), Some(pubkey)) => {
                self.process_claims_delete(pubkey, value).await
            }
            (operation, Some("REFMAP"), id) => {
                // TODO: process REFMAP entries
                debug!(?operation, id, "ignoring REFMAP entry");
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
#[derive(Debug, Default, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_schema: Option<String>,
}

impl TryFrom<Claims> for StoredClaims {
    type Error = anyhow::Error;

    fn try_from(claims: Claims) -> Result<Self, Self::Error> {
        match claims {
            Claims::Actor(jwt::Claims {
                issuer,
                subject,
                metadata,
                ..
            }) => {
                let jwt::Actor {
                    name,
                    tags,
                    caps,
                    rev,
                    ver,
                    call_alias,
                    ..
                } = metadata.context("no metadata found on actor claims")?;
                Ok(StoredClaims {
                    call_alias: call_alias.unwrap_or_default(),
                    capabilities: caps.unwrap_or_default().join(","),
                    issuer,
                    name: name.unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject,
                    tags: tags.unwrap_or_default().join(","),
                    version: ver.unwrap_or_default(),
                    ..Default::default()
                })
            }
            Claims::Provider(jwt::Claims {
                issuer,
                subject,
                metadata,
                ..
            }) => {
                let jwt::CapabilityProvider {
                    name,
                    capid: contract_id,
                    rev,
                    ver,
                    config_schema,
                    ..
                } = metadata.context("no metadata found on provider claims")?;
                Ok(StoredClaims {
                    contract_id,
                    issuer,
                    name: name.unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject,
                    version: ver.unwrap_or_default(),
                    config_schema: config_schema.map(|schema| schema.to_string()),
                    ..Default::default()
                })
            }
        }
    }
}

impl TryFrom<&Claims> for StoredClaims {
    type Error = anyhow::Error;

    fn try_from(claims: &Claims) -> Result<Self, Self::Error> {
        match claims {
            Claims::Actor(jwt::Claims {
                issuer,
                subject,
                metadata,
                ..
            }) => {
                let jwt::Actor {
                    name,
                    tags,
                    caps,
                    rev,
                    ver,
                    call_alias,
                    ..
                } = metadata
                    .as_ref()
                    .context("no metadata found on actor claims")?;
                Ok(StoredClaims {
                    call_alias: call_alias.clone().unwrap_or_default(),
                    capabilities: caps.clone().unwrap_or_default().join(","),
                    issuer: issuer.clone(),
                    name: name.clone().unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject: subject.clone(),
                    tags: tags.clone().unwrap_or_default().join(","),
                    version: ver.clone().unwrap_or_default(),
                    ..Default::default()
                })
            }
            Claims::Provider(jwt::Claims {
                issuer,
                subject,
                metadata,
                ..
            }) => {
                let jwt::CapabilityProvider {
                    name,
                    capid: contract_id,
                    rev,
                    ver,
                    config_schema,
                    ..
                } = metadata
                    .as_ref()
                    .context("no metadata found on provider claims")?;
                Ok(StoredClaims {
                    contract_id: contract_id.clone(),
                    issuer: issuer.clone(),
                    name: name.clone().unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject: subject.clone(),
                    version: ver.clone().unwrap_or_default(),
                    config_schema: config_schema.as_ref().map(ToString::to_string),
                    ..Default::default()
                })
            }
        }
    }
}

fn human_friendly_uptime(uptime: Duration) -> String {
    // strip sub-seconds, then convert to human-friendly format
    humantime::format_duration(
        uptime.saturating_sub(Duration::from_nanos(uptime.subsec_nanos().into())),
    )
    .to_string()
}

/// Ensure actor has the capability claim to send or receive this invocation. This
/// should be called whenever an actor is about to send or receive an invocation.
fn ensure_actor_capability(
    claims_metadata: Option<&jwt::Actor>,
    contract_id: impl AsRef<str>,
) -> anyhow::Result<()> {
    let contract_id = contract_id.as_ref();
    match claims_metadata {
        // [ADR-0006](https://github.com/wasmCloud/wasmCloud/blob/main/adr/0006-actor-to-actor.md)
        // Allow actor to actor calls by default
        _ if contract_id.is_empty() => {}
        Some(jwt::Actor {
            caps: Some(ref caps),
            ..
        }) => {
            ensure!(
                caps.iter().any(|cap| cap == contract_id),
                "actor does not have capability claim `{contract_id}`"
            );
        }
        Some(_) | None => bail!("actor missing capability claims, denying invocation"),
    }
    Ok(())
}

fn injector_to_headers(injector: &TraceContextInjector) -> async_nats::header::HeaderMap {
    injector
        .iter()
        .filter_map(|(k, v)| {
            // There's not really anything we can do about headers that don't parse
            let name = async_nats::header::HeaderName::from_str(k.as_str()).ok()?;
            let value = async_nats::header::HeaderValue::from_str(v.as_str()).ok()?;
            Some((name, value))
        })
        .collect()
}

#[cfg(test)]
mod test {
    use nkeys::KeyPair;
    use ulid::Ulid;
    use uuid::Uuid;
    use wascap::jwt;
    use wasmcloud_core::{invocation_hash, WasmCloudEntity};
    use wasmcloud_tracing::context::TraceContextInjector;

    use super::Invocation;

    const CLUSTER_PUBKEY: &str = "CAQQHYABXBPDBZIGDZIT7E73HW66RPCFC3GGLQKSDDTVWUVOYZBYHUND";
    const CLUSTER_SEED: &str = "SCAIYCZTW775GJYX3MVWLURALVC3PULW43PTEKGH72JBMA3A7LOLGLQ2JA";
    const HOSTKEY_PUBKEY: &str = "NAGASFIHF5SBGTNTZUOVWT3O63SJUMIFUL7BH52T3UAFKQW7PPP46KFU";
    const HOSTKEY_SEED: &str = "SNAAGG2LW2FP2JTXDTLQNB4EOHQDHQR2Y75PYAVOCYUPGHUK24WA3RSZYI";
    const ACTOR_PUBKEY: &str = "MDNX3CB6VBXG55GOJ6UYON7AMK6SLYPB6GLPRZGTEE6625EFLJDQKWWR";
    const PROVIDER_PUBKEY: &str = "VC3IJSRK3KIJUD5PQIEU2UNWT4PQCRYTAXFC4PDLTCMDX7L77YRUGCXW";
    const OUTSIDE_CLUSTER_PUBKEY: &str = "CAT4QMKWIUTIX5ZBNOT2ICJHCSVVHGHLOHSXDSS5P2MIWRXHYHANTJZQ";

    #[test]
    #[allow(clippy::too_many_lines)]
    fn validate_antiforgery_catches_invalid_invocations() {
        let clusterkey = KeyPair::from_seed(CLUSTER_SEED).expect("failed to create cluster key");
        assert_eq!(clusterkey.public_key(), CLUSTER_PUBKEY);
        let hostkey = KeyPair::from_seed(HOSTKEY_SEED).expect("failed to create host key");
        assert_eq!(hostkey.public_key(), HOSTKEY_PUBKEY);
        let origin = actor_entity(ACTOR_PUBKEY);
        let target = provider_entity(PROVIDER_PUBKEY, "default", "wasmcloud:testoperation");
        let operation = "wasmcloud:bus/TestOperation.HandleTest";
        let msg = vec![0xF0, 0x9F, 0x8C, 0xAE];

        let target_operation_url = format!("{}/TestOperation.HandleTest", target.url());
        let valid_issuers = vec![CLUSTER_PUBKEY.to_string()];

        let basic_invocation: Invocation = Invocation::new(
            &clusterkey,
            &hostkey,
            origin.clone(),
            target.clone(),
            operation.to_string(),
            msg.clone(),
            TraceContextInjector::default_with_span().into(),
        )
        .expect("failed to create invocation");
        assert!(basic_invocation
            .validate_antiforgery(&valid_issuers)
            .is_ok());
        // Ensure issuer signed with a key that isn't in the list of valid issuers is rejected
        assert!(basic_invocation
            .validate_antiforgery(&[OUTSIDE_CLUSTER_PUBKEY.to_string()])
            .is_err_and(|e| e
                .to_string()
                .contains("issuer of this invocation is not among the list of valid issuers")));

        // Ensure claims that are expired are rejected
        let old_claims = jwt::Claims::<jwt::Invocation>::with_dates(
            CLUSTER_PUBKEY.to_string(),
            Uuid::from_u128(Ulid::new().into()).to_string(),
            Some(0),
            Some(0),
            &target.url(),
            &origin.url(),
            &invocation_hash(&target_operation_url, origin.url(), operation, msg.clone()),
        );
        let old_invocation = Invocation {
            encoded_claims: old_claims
                .encode(&clusterkey)
                .expect("failed to encode old claims"),
            ..basic_invocation.clone()
        };
        assert!(old_invocation
            .validate_antiforgery(&valid_issuers)
            .is_err_and(|e| e.to_string().contains("invocation claims token expired")));

        // Ensure claims that aren't valid yet are rejected
        let nbf_claims = jwt::Claims::<jwt::Invocation>::with_dates(
            CLUSTER_PUBKEY.to_string(),
            Uuid::from_u128(Ulid::new().into()).to_string(),
            Some(u64::MAX),
            Some(u64::MAX),
            &target_operation_url,
            &origin.url(),
            &invocation_hash(&target_operation_url, origin.url(), operation, msg.clone()),
        );
        let nbf_invocation = Invocation {
            encoded_claims: nbf_claims
                .encode(&clusterkey)
                .expect("failed to encode old claims"),
            ..basic_invocation.clone()
        };
        assert!(nbf_invocation
            .validate_antiforgery(&valid_issuers)
            .is_err_and(|e| e
                .to_string()
                .contains("attempt to use invocation before claims token allows")));

        // Ensure invocations that don't match the claims hash are rejected
        let bad_hash_invocation = Invocation {
            msg: vec![0xF0, 0x9F, 0x98, 0x88],
            ..basic_invocation.clone()
        };
        assert!(bad_hash_invocation
            .validate_antiforgery(&valid_issuers)
            .is_err_and(|e| e
                .to_string()
                .contains("invocation hash does not match signed claims hash")));

        // Ensure mismatched host IDs are rejected
        let bad_host_invocation = Invocation {
            host_id: "NOTAHOSTID".to_string(),
            ..basic_invocation.clone()
        };
        assert!(bad_host_invocation
            .validate_antiforgery(&valid_issuers)
            .is_err_and(|e| e
                .to_string()
                .contains("invalid host ID on invocation: 'NOTAHOSTID'")));

        // Ensure mismatched origin URL is rejected
        let bad_origin_claims = jwt::Claims::<jwt::Invocation>::new(
            CLUSTER_PUBKEY.to_string(),
            Uuid::from_u128(Ulid::new().into()).to_string(),
            &target_operation_url,
            &origin.url(),
            &invocation_hash(&target_operation_url, origin.url(), operation, msg.clone()),
        );
        let bad_origin_invocation = Invocation {
            encoded_claims: bad_origin_claims
                .encode(&clusterkey)
                .expect("failed to encode claims"),
            origin: actor_entity("somethingelse"),
            ..basic_invocation.clone()
        };
        assert!(bad_origin_invocation
            .validate_antiforgery(&valid_issuers)
            .is_err_and(|e| e
                .to_string()
                .contains("invocation claims and invocation origin URL do not match")));

        // Ensure mismatched target URL is rejected (setting a different operation affects target URL)
        let bad_target_invocation = Invocation {
            operation: "wasmcloud:bus/Bitcoin.MineBitcoin".to_string(),
            ..basic_invocation.clone()
        };
        assert!(bad_target_invocation
            .validate_antiforgery(&valid_issuers)
            .is_err_and(|e| e
                .to_string()
                .contains("invocation claims and invocation target URL do not match")));
    }

    /// Helper test function for oneline creation of an actor [`WasmCloudEntity`]. Consider adding to the
    /// actual impl block if it's useful elsewhere.
    fn actor_entity(public_key: &str) -> WasmCloudEntity {
        WasmCloudEntity {
            public_key: public_key.to_string(),
            ..Default::default()
        }
    }

    /// Helper test function for oneline creation of a provider [`WasmCloudEntity`]. Consider adding to the
    /// actual impl block if it's useful elsewhere.
    fn provider_entity(public_key: &str, link_name: &str, contract_id: &str) -> WasmCloudEntity {
        WasmCloudEntity {
            public_key: public_key.to_string(),
            link_name: link_name.to_string(),
            contract_id: contract_id.to_string(),
        }
    }
}
