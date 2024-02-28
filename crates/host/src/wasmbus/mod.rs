use core::future::Future;
use core::num::NonZeroUsize;
use core::ops::{Deref, RangeInclusive};
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use std::collections::hash_map::{self, Entry};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::env::consts::{ARCH, FAMILY, OS};
use std::io::Cursor;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context as ErrContext};
use async_nats::jetstream::kv::{Entry as KvEntry, Operation, Store};
use async_nats::Subscriber;
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::{BufMut, Bytes, BytesMut};
use cloudevents::{EventBuilder, EventBuilderV10};
use futures::stream::{AbortHandle, Abortable, SelectAll};
use futures::{
    future::Either, join, stream, try_join, Stream, StreamExt, TryFutureExt, TryStreamExt,
};
use nkeys::{KeyPair, KeyPairType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{empty, stderr, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{interval_at, Instant};
use tokio::{process, select, spawn};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn};
use uuid::Uuid;
use wasmcloud_tracing::{global, KeyValue};

pub use config::Host as HostConfig;
use wascap::{jwt, prelude::ClaimsBuilder};
use wasmcloud_control_interface::{
    ActorAuctionAck, ActorAuctionRequest, ActorDescription, CtlResponse,
    DeleteInterfaceLinkDefinitionRequest, GetClaimsResponse, HostInventory, HostLabel,
    InterfaceLinkDefinition, ProviderAuctionAck, ProviderAuctionRequest, ProviderDescription,
    RegistryCredential, ScaleActorCommand, StartProviderCommand, StopHostCommand,
    StopProviderCommand, UpdateActorCommand, WitInterface,
};
use wasmcloud_core::{
    HealthCheckResponse, HostData, Invocation, InvocationResponse, LatticeTargetId, LinkName,
    OtelConfig, WasmCloudEntity,
};
use wasmcloud_runtime::capability::logging::logging;
use wasmcloud_runtime::capability::{
    blobstore, guest_config, messaging, Blobstore, Bus, CallTargetInterface, KeyValueAtomic,
    KeyValueEventual, Logging, Messaging, OutgoingHttp, OutgoingHttpRequest, TargetEntity,
    WrpcInterfaceTarget,
};
use wasmcloud_runtime::Runtime;
use wasmcloud_tracing::context::TraceContextInjector;
use wrpc_transport::{AcceptedInvocation, Client, Transmitter};
use wrpc_types::DynamicFunction;

use crate::{
    fetch_actor, HostMetrics, OciConfig, PolicyAction, PolicyHostInfo, PolicyManager,
    PolicyRequestTarget, PolicyResponse, RegistryAuth, RegistryConfig, RegistryType,
};

/// wasmCloud host configuration
pub mod config;

mod event;

const WRPC: &str = "wrpc";
const WRPC_VERSION: &str = "0.0.1";

#[derive(Debug)]
struct Queue {
    all_streams: SelectAll<Subscriber>,
}

impl Stream for Queue {
    type Item = async_nats::Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.all_streams.poll_next_unpin(cx)
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
        lattice: &str,
        cluster_key: &KeyPair,
        host_key: &KeyPair,
    ) -> anyhow::Result<Self> {
        let host_id = host_key.public_key();
        let streams = futures::future::join_all([
            Either::Left(nats.subscribe(format!("{topic_prefix}.{lattice}.registry.put",))),
            Either::Left(nats.subscribe(format!("{topic_prefix}.{lattice}.host.ping",))),
            Either::Left(nats.subscribe(format!("{topic_prefix}.{lattice}.*.auction",))),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{lattice}.link.*"),
                format!("{topic_prefix}.{lattice}.link",),
            )),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{lattice}.claims.get"),
                format!("{topic_prefix}.{lattice}.claims"),
            )),
            Either::Left(nats.subscribe(format!("{topic_prefix}.{lattice}.actor.*.{host_id}"))),
            Either::Left(nats.subscribe(format!("{topic_prefix}.{lattice}.provider.*.{host_id}"))),
            Either::Left(nats.subscribe(format!("{topic_prefix}.{lattice}.label.*.{host_id}"))),
            Either::Left(nats.subscribe(format!("{topic_prefix}.{lattice}.host.*.{host_id}"))),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{lattice}.config.>"),
                format!("{topic_prefix}.{lattice}.config"),
            )),
        ])
        .await
        .into_iter()
        .collect::<Result<Vec<_>, async_nats::SubscribeError>>()
        .context("failed to subscribe to queues")?;
        Ok(Self {
            all_streams: futures::stream::select_all(streams),
        })
    }
}

#[derive(Clone, Debug)]
struct Handler {
    nats: async_nats::Client,
    config_data: Arc<RwLock<ConfigCache>>,
    lattice: String,
    cluster_key: Arc<KeyPair>,
    host_key: Arc<KeyPair>,
    claims: jwt::Claims<jwt::Actor>,
    origin: WasmCloudEntity,
    /// The current link name to use for interface targets, overridable in actor code via set_target()
    interface_link_name: Arc<RwLock<LinkName>>,

    /// Map of link names -> WIT ns & package -> WIT interface -> Target
    ///
    /// While a target may often be a component ID, it is not guaranteed to be one, and could be
    /// some other identifier of where to send invocations, representing one or more lattice entities.
    ///
    /// Lattice entities could be:
    /// - A (single) Component ID
    /// - A routing group
    /// - Some other opaque string
    #[allow(clippy::type_complexity)]
    interface_links:
        Arc<RwLock<HashMap<LinkName, HashMap<String, HashMap<WitInterface, LatticeTargetId>>>>>,
    /// Map of interface -> function name -> function result types
    ///
    /// When invoking a function that the component imports, this map is consulted to determine the
    /// result types of the function, which is required for the wRPC protocol to set up proper
    /// subscriptions for the return types.
    polyfilled_imports: HashMap<String, HashMap<String, Arc<[wrpc_types::Type]>>>,
}

#[instrument(level = "trace")]
// TODO(#1548): This function should be removed, we should invoke wRPC targets not WasmCloudEntities
async fn resolve_target(target: Option<&TargetEntity>) -> anyhow::Result<WasmCloudEntity> {
    trace!("resolve target");

    let target = match target {
        Some(TargetEntity::Wrpc(target)) => {
            let (namespace, package, _, _) = target.interface.as_parts();
            WasmCloudEntity {
                public_key: target.id.clone(),
                contract_id: format!("{namespace}:{package}"),
                link_name: target.link_name.clone(),
            }
        }
        _ => bail!("target entity was not resolvable to a wasmcloud entity target"),
    };
    Ok(target)
}

impl Handler {
    #[instrument(level = "debug", skip(self, operation, request))]
    async fn call_operation_with_payload(
        &self,
        target: Option<TargetEntity>,
        operation: impl Into<String>,
        request: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, String>> {
        let operation = operation.into();
        // Determine the target for the operation
        let (_, interface_and_func) = operation
            .rsplit_once('/')
            .context("failed to parse operation")?;
        let inv_target = resolve_target(target.as_ref()).await?;
        let injector = TraceContextInjector::default_with_span();
        let headers = injector_to_headers(&injector);
        let invocation = Invocation::new(
            &self.cluster_key,
            &self.host_key,
            self.origin.clone(),
            inv_target.clone(),
            operation.clone(),
            request,
            injector.into(),
        )?;

        let payload =
            rmp_serde::to_vec_named(&invocation).context("failed to encode invocation")?;

        // Determine the subject on which to transmit
        let subject = match target {
            Some(TargetEntity::Wrpc(target)) => {
                let (namespace, package, interface, _) = target.interface.as_parts();
                let (_, function) = interface_and_func
                    .split_once('.')
                    .context("interface and function should be specified")?;
                format!(
                    "{}.{}.{WRPC}.{WRPC_VERSION}.{}:{}/{}.{}",
                    self.lattice, target.id, namespace, package, interface, function
                )
            }
            // TODO: just remove other options entirely
            _ => bail!("target entity was not resolvable to a WRPC target"),
        };

        let request = async_nats::Request::new()
            .payload(payload.into())
            .headers(headers); // TODO: remove headers once all providers are built off the new SDK, which parses the trace context in the invocation
        let res = self
            .nats
            .send_request(subject, request)
            .await
            .context("failed to publish on NATS subject")?;

        // Process response
        let InvocationResponse {
            invocation_id,
            msg,
            error,
            ..
        } = rmp_serde::from_slice(&res.payload).context("failed to decode invocation response")?;
        ensure!(invocation_id == invocation.id, "invocation ID mismatch");

        if let Some(error) = error {
            Ok(Err(error))
        } else {
            Ok(Ok(msg))
        }
    }

    #[instrument(level = "debug", skip(self, operation, request))]
    async fn call_operation(
        &self,
        target: Option<TargetEntity>,
        operation: impl Into<String>,
        request: &impl Serialize,
    ) -> anyhow::Result<Vec<u8>> {
        // TODO(brooksmtownsend): This handler should be sending requests over wRPC, rather than creating
        // invocations.
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
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        self.call_operation(
            target,
            "wasmcloud:blobstore/Blobstore.CreateContainer",
            &name,
        )
        .await
        .and_then(decode_empty_provider_response)
    }

    #[instrument]
    async fn container_exists(&self, name: &str) -> anyhow::Result<bool> {
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        self.call_operation(
            target,
            "wasmcloud:blobstore/Blobstore.ContainerExists",
            &name,
        )
        .await
        .and_then(decode_provider_response)
    }

    #[instrument]
    async fn delete_container(&self, name: &str) -> anyhow::Result<()> {
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        self.call_operation(
            target,
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
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        let res = self
            .call_operation(
                target,
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
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        let res = self
            .call_operation(
                target,
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
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        self.call_operation(
            target,
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
        let mut bytes = Vec::new();
        value
            .read_to_end(&mut bytes)
            .await
            .context("failed to read bytes")?;
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        let res = self
            .call_operation(
                target,
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
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        let res = self
            .call_operation(
                target,
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
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        let res = self
            .call_operation(
                target,
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
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )))
            .await?;
        let res = self
            .call_operation(target, "wasmcloud:blobstore/Blobstore.GetObjectInfo", &name)
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
    #[instrument(level = "trace", skip(self))]
    async fn identify_interface_target(
        &self,
        target_interface: &CallTargetInterface,
    ) -> anyhow::Result<Option<TargetEntity>> {
        let links = self.interface_links.read().await;
        let link_name = self.interface_link_name.read().await.clone();
        let (namespace, package, interface, _) = target_interface.as_parts();

        // Determine the lattice target ID we should be sending to
        let lattice_target_id = links
            .get(self.interface_link_name.read().await.as_str())
            .and_then(|packages| packages.get(&format!("{namespace}:{package}")))
            .and_then(|interfaces| interfaces.get(interface));

        // If we managed to find a target ID, convert it into an entity
        let target_entity = lattice_target_id.map(|id| {
            TargetEntity::Wrpc(WrpcInterfaceTarget {
                id: id.clone(),
                interface: target_interface.clone(),
                link_name,
            })
        });
        Ok(target_entity)
    }

    /// Get the current link name
    #[instrument(level = "debug", skip_all)]
    async fn get_link_name(&self) -> anyhow::Result<String> {
        Ok(self.interface_link_name.read().await.deref().clone())
    }

    /// Set the current link name in use by the handler, which is otherwise "default".
    ///
    /// Link names are important to set to differentiate similar operations (ex. `wasi:keyvalue/readwrite.get`)
    /// that should go to different targets (ex. a capability provider like `kv-redis` vs `kv-vault`)
    #[instrument(level = "debug", skip(self))]
    async fn set_link_name(
        &self,
        link_name: LinkName,
        _interfaces: Vec<CallTargetInterface>,
    ) -> anyhow::Result<()> {
        *self.interface_link_name.write().await = link_name;
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, guest_config::ConfigError>> {
        let conf = self.config_data.read().await;
        let data = conf
            .get(&self.claims.subject)
            .and_then(|conf| conf.get(key))
            .cloned()
            .map(|val| val.into_bytes());
        Ok(Ok(data))
    }

    #[instrument(level = "debug", skip_all)]
    async fn get_all(
        &self,
    ) -> anyhow::Result<Result<Vec<(String, Vec<u8>)>, guest_config::ConfigError>> {
        Ok(Ok(self
            .config_data
            .read()
            .await
            .get(&self.claims.subject)
            .cloned()
            .map(|conf| {
                conf.into_iter()
                    .map(|(key, val)| (key, val.into_bytes()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()))
    }

    #[instrument(level = "debug", skip(self, params))]
    async fn call(
        &self,
        target: Option<TargetEntity>,
        instance: &str,
        name: &str,
        params: Vec<wrpc_transport::Value>,
    ) -> anyhow::Result<Vec<wrpc_transport::Value>> {
        if let Some(TargetEntity::Wrpc(interface_target)) = target {
            let result_types = match self
                .polyfilled_imports
                .get(instance)
                .and_then(|functions| functions.get(name))
            {
                Some(results) => results.as_ref(),
                None => bail!(
                    "polyfilled import {}/{} not found, could not determine result types",
                    instance,
                    name
                ),
            };
            let prefix = format!("{}.{}", self.lattice, interface_target.id);
            let wrpc_client =
                wrpc_transport_nats::Client::new(self.nats.clone(), prefix.to_string());
            let (result, _tx) = wrpc_client
                .invoke_dynamic(instance, name, params, result_types)
                .await?;

            Ok(result)
        } else {
            error!("invalid target");
            Ok(vec![])
        }
    }
}

#[async_trait]
impl KeyValueAtomic for Handler {
    #[instrument(skip(self))]
    async fn increment(&self, bucket: &str, key: String, delta: u64) -> anyhow::Result<u64> {
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let value = delta.try_into().context("delta does not fit in `i32`")?;
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi", "keyvalue", "atomic", None,
            )))
            .await?;
        let res = self
            .call_operation(
                target,
                "wasmcloud:keyvalue/KeyValue.Increment",
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
impl KeyValueEventual for Handler {
    #[instrument(skip(self))]
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<Option<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>> {
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi", "keyvalue", "eventual", None,
            )))
            .await?;
        let res = self
            .call_operation(target, "wasmcloud:keyvalue/KeyValue.Get", &key)
            .await?;
        let wasmcloud_compat::keyvalue::GetResponse { value, exists } =
            decode_provider_response(res)?;
        if !exists {
            return Ok(None);
        }
        let size = value
            .len()
            .try_into()
            .context("value size does not fit in `u64`")?;
        Ok(Some((Box::new(Cursor::new(value)), size)))
    }

    #[instrument(skip(self, value))]
    async fn set(
        &self,
        bucket: &str,
        key: String,
        mut value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()> {
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let mut buf = String::new();
        value
            .read_to_string(&mut buf)
            .await
            .context("failed to read value")?;
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi", "keyvalue", "eventual", None,
            )))
            .await?;
        self.call_operation(
            target,
            "wasmcloud:keyvalue/KeyValue.Set",
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
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi", "keyvalue", "eventual", None,
            )))
            .await?;
        let res = self
            .call_operation(target, "wasmcloud:keyvalue/KeyValue.Del", &key)
            .await?;
        let deleted: bool = decode_provider_response(res)?;
        ensure!(deleted, "key not found");
        Ok(())
    }

    #[instrument(skip(self))]
    async fn exists(&self, bucket: &str, key: String) -> anyhow::Result<bool> {
        if !bucket.is_empty() {
            bail!("buckets not currently supported")
        }
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi", "keyvalue", "eventual", None,
            )))
            .await?;
        self.call_operation(target, "wasmcloud:keyvalue/KeyValue.Contains", &key)
            .await
            .and_then(decode_provider_response)
    }
}

#[async_trait]
impl Logging for Handler {
    #[instrument(level = "debug", skip_all)]
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
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
        let timeout_ms = timeout
            .as_millis()
            .try_into()
            .context("timeout milliseconds do not fit in `u32`")?;
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasmcloud",
                "messaging",
                "consumer",
                None,
            )))
            .await?;
        let res = self
            .call_operation(
                target,
                "wasmcloud:messaging/Messaging.Request",
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
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasmcloud",
                "messaging",
                "consumer",
                None,
            )))
            .await?;
        self.call_operation(
            target,
            "wasmcloud:messaging/Messaging.Publish",
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

#[async_trait]
impl OutgoingHttp for Handler {
    #[instrument(skip_all)]
    async fn handle(
        &self,
        OutgoingHttpRequest {
            use_tls: _,
            authority: _,
            request,
            connect_timeout: _,
            first_byte_timeout: _,
            between_bytes_timeout: _,
        }: OutgoingHttpRequest,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        let req = wasmcloud_compat::HttpClientRequest::from_http(request)
            .await
            .context("failed to convert HTTP request")?;
        let target = self
            .identify_interface_target(&CallTargetInterface::from_parts((
                "wasi",
                "http",
                "outgoing-handler",
                None,
            )))
            .await?;
        let res = self
            .call_operation(target, "wasmcloud:httpclient/HttpClient.Request", &req)
            .await?;
        let res: wasmcloud_compat::HttpResponse = decode_provider_response(res)?;
        let res = ::http::Response::<Vec<u8>>::try_from(res)?;
        Ok(res.map(|body| -> Box<dyn AsyncRead + Sync + Send + Unpin> {
            Box::new(Cursor::new(body))
        }))
    }
}

type Annotations = BTreeMap<String, String>;

#[derive(Debug)]
struct Actor {
    component: wasmcloud_runtime::Component,
    /// Unique component identifier for this actor
    id: String,
    calls: AbortHandle,
    handler: Handler,
    annotations: Annotations,
    /// Maximum number of instances of this actor that can be running at once
    max_instances: NonZeroUsize,
    image_reference: String,
    metrics: Arc<HostMetrics>,
    // TODO(#1220): implement issuer verification
    /// Cluster issuers that this actor should accept invocations from
    #[allow(unused)]
    valid_issuers: Vec<String>,
    // TODO(#1548): ensure we are validating actor start and invocations
    #[allow(unused)]
    policy_manager: Arc<PolicyManager>,
    // TODO: use a single map once Claims is an enum
    // TODO(#1548): make optional
    #[allow(unused)]
    actor_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::Actor>>>>,
}

impl Deref for Actor {
    type Target = wasmcloud_runtime::Component;

    fn deref(&self) -> &Self::Target {
        &self.component
    }
}

impl Actor {
    /// Returns the component specification for this unique actor
    pub(crate) async fn component_specification(&self) -> ComponentSpecification {
        let mut spec = ComponentSpecification::new(&self.image_reference);
        spec.links = self.handler.interface_links.read().await.clone();
        spec
    }

    /// Handle an incoming wRPC request to invoke an export on this actor instance.
    async fn handle_wrpc_call(
        &self,
        instance: &str,
        name: &str,
        inv_params: Vec<wrpc_transport::Value>,
        response_subject: wrpc_transport_nats::Subject,
        transmitter: &wrpc_transport_nats::Transmitter,
    ) -> anyhow::Result<()> {
        // TODO(#1548): implement querying policy server

        // Instantiate component with expected handlers
        let mut component_instance = self
            .instantiate()
            .expect("should be able to instantiate actor");
        component_instance
            .stderr(stderr())
            .await
            .context("failed to set stderr")?
            .blobstore(Arc::new(self.handler.clone()))
            .bus(Arc::new(self.handler.clone()))
            .keyvalue_atomic(Arc::new(self.handler.clone()))
            .keyvalue_eventual(Arc::new(self.handler.clone()))
            .logging(Arc::new(self.handler.clone()))
            .messaging(Arc::new(self.handler.clone()))
            .outgoing_http(Arc::new(self.handler.clone()));

        let start_at = Instant::now();
        let response = component_instance.call(instance, name, inv_params).await;
        // Record metric on duration of the call
        let elapsed = u64::try_from(start_at.elapsed().as_nanos()).unwrap_or_default();

        let mut attributes = vec![
            KeyValue::new("actor.ref", self.image_reference.clone()),
            KeyValue::new("interface", instance.clone()),
            KeyValue::new("function", name.clone()),
            KeyValue::new("lattice", self.metrics.lattice_id.clone()),
            KeyValue::new("host", self.metrics.host_id.clone()),
        ];
        // TODO: add in attributes about the source
        self.metrics
            .handle_rpc_message_duration_ns
            .record(elapsed, &attributes);
        self.metrics.actor_invocations.add(1, &attributes);
        match response {
            Ok(resp) => {
                transmitter
                    .transmit_tuple_dynamic(response_subject, resp)
                    .await
            }
            Err(e) => {
                error!("failed to handle wrpc request: {e}");
                self.metrics.actor_errors.add(1, &attributes);
                Ok(())
            }
        }
    }
}

#[derive(Debug)]
struct Provider {
    child: JoinHandle<()>,
    annotations: Annotations,
    image_ref: String,
    /// TODO(#1548): optional claims
    claims: jwt::Claims<jwt::CapabilityProvider>,
}

type ConfigCache = HashMap<String, HashMap<String, String>>;

/// wasmCloud Host
pub struct Host {
    // TODO: Clean up actors after stop
    /// The actor map is a map of actor component ID to actor
    actors: RwLock<HashMap<String, Arc<Actor>>>,
    cluster_key: Arc<KeyPair>,
    cluster_issuers: Vec<String>,
    event_builder: EventBuilderV10,
    friendly_name: String,
    heartbeat: AbortHandle,
    host_config: HostConfig,
    host_key: Arc<KeyPair>,
    labels: RwLock<HashMap<String, String>>,
    ctl_topic_prefix: String,
    /// NATS client to use for control interface subscriptions and jetstream queries
    ctl_nats: async_nats::Client,
    /// NATS client to use for RPC calls
    rpc_nats: async_nats::Client,
    data: Store,
    data_watch: AbortHandle,
    config_data: Store,
    config_data_watch: AbortHandle,
    policy_manager: Arc<PolicyManager>,
    /// The provider map is a map of provider component ID to provider
    providers: RwLock<HashMap<String, Provider>>,
    registry_config: RwLock<HashMap<String, RegistryConfig>>,
    runtime: Runtime,
    start_at: Instant,
    stop_tx: watch::Sender<Option<Instant>>,
    stop_rx: watch::Receiver<Option<Instant>>,
    queue: AbortHandle,
    // Component ID -> All Links
    links: RwLock<HashMap<String, HashSet<InterfaceLinkDefinition>>>,
    actor_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::Actor>>>>, // TODO: use a single map once Claims is an enum
    provider_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::CapabilityProvider>>>>,
    config_data_cache: Arc<RwLock<ConfigCache>>,
    metrics: Arc<HostMetrics>,
}

#[derive(Debug, Serialize, Deserialize)]
/// The specification of a component that is or did run in the lattice. This contains all of the information necessary to
/// instantiate a component in the lattice (url and digest) as well as configuration and links in order to facilitate
/// runtime execution of the component. Each `import` in a component's WIT world will need a corresponding link for the
/// host runtime to route messages to the correct component.
pub struct ComponentSpecification {
    /// The URL of the component, file, OCI, or otherwise
    url: String,
    /// All outbound links from this component to other components, used for routing when calling a component `import`
    links: HashMap<String, HashMap<String, HashMap<String, String>>>,
    ////
    // Possible additions in the future, left in as comments to facilitate discussion
    ////
    // /// The claims embedded in the component, if present
    // claims: Option<Claims>,
    // /// SHA256 digest of the component, used for checking uniqueness of component IDs
    // digest: String
    // /// (Advanced) Additional routing topics to subscribe on in addition to the component ID.
    // routing_groups: Vec<String>,
}

impl ComponentSpecification {
    /// Create a new empty component specification with the given ID and URL
    pub fn new(url: impl AsRef<str>) -> Self {
        Self {
            url: url.as_ref().to_string(),
            links: HashMap::new(),
        }
    }
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
        match claims {
            StoredClaims::Actor(claims) => {
                let name = (!claims.name.is_empty()).then_some(claims.name);
                let rev = claims.revision.parse().ok();
                let ver = (!claims.version.is_empty()).then_some(claims.version);
                let tags = (!claims.tags.is_empty()).then_some(claims.tags);
                let caps = (!claims.capabilities.is_empty()).then_some(claims.capabilities);
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
            }
            StoredClaims::Provider(claims) => {
                let name = (!claims.name.is_empty()).then_some(claims.name);
                let rev = claims.revision.parse().ok();
                let ver = (!claims.version.is_empty()).then_some(claims.version);
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
}

#[instrument(level = "debug", skip_all)]
async fn create_bucket(
    jetstream: &async_nats::jetstream::Context,
    bucket: &str,
) -> anyhow::Result<Store> {
    // Don't create the bucket if it already exists
    if let Ok(store) = jetstream.get_key_value(bucket).await {
        info!(%bucket, "bucket already exists. Skipping creation.");
        return Ok(store);
    }

    match jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: bucket.to_string(),
            ..Default::default()
        })
        .await
    {
        Ok(store) => {
            info!(%bucket, "created bucket with 1 replica");
            Ok(store)
        }
        Err(err) => Err(anyhow!(err).context(format!("failed to create bucket '{bucket}'"))),
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

#[instrument(level = "debug", skip_all)]
async fn load_supplemental_config(
    ctl_nats: &async_nats::Client,
    lattice: &str,
    labels: &HashMap<String, String>,
) -> anyhow::Result<SupplementalConfig> {
    #[derive(Deserialize, Default)]
    struct SerializedSupplementalConfig {
        #[serde(default, rename = "registryCredentials")]
        registry_credentials: Option<HashMap<String, RegistryCredential>>,
    }

    let cfg_topic = format!("wasmbus.cfg.{lattice}.req");
    let cfg_payload = serde_json::to_vec(&json!({
        "labels": labels,
    }))
    .context("failed to serialize config payload")?;

    debug!("requesting supplemental config");
    match ctl_nats.request(cfg_topic, cfg_payload.into()).await {
        Ok(resp) => {
            match serde_json::from_slice::<SerializedSupplementalConfig>(resp.payload.as_ref()) {
                Ok(ser_cfg) => Ok(SupplementalConfig {
                    registry_config: ser_cfg.registry_credentials.map(|creds| {
                        creds
                            .into_iter()
                            .map(|(k, v)| {
                                debug!(registry_url = %k, "set registry config");
                                (k, v.into())
                            })
                            .collect()
                    }),
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

#[instrument(level = "debug", skip_all)]
async fn merge_registry_config(
    registry_config: &RwLock<HashMap<String, RegistryConfig>>,
    oci_opts: OciConfig,
) -> () {
    let mut registry_config = registry_config.write().await;
    let allow_latest = oci_opts.allow_latest;

    // update auth for specific registry, if provided
    if let Some(reg) = oci_opts.oci_registry {
        match registry_config.entry(reg.clone()) {
            Entry::Occupied(_entry) => {
                // note we don't update config here, since the config service should take priority
                warn!(oci_registry_url = %reg, "ignoring OCI registry config, overriden by config service");
            }
            Entry::Vacant(entry) => {
                debug!(oci_registry_url = %reg, "set registry config");
                entry.insert(RegistryConfig {
                    reg_type: RegistryType::Oci,
                    auth: RegistryAuth::from((oci_opts.oci_user, oci_opts.oci_password)),
                    ..Default::default()
                });
            }
        }
    }

    // update or create entry for all registries in allowed_insecure
    oci_opts.allowed_insecure.into_iter().for_each(|reg| {
        match registry_config.entry(reg.clone()) {
            Entry::Occupied(mut entry) => {
                debug!(oci_registry_url = %reg, "set allowed_insecure");
                entry.get_mut().allow_insecure = true;
            }
            Entry::Vacant(entry) => {
                debug!(oci_registry_url = %reg, "set allowed_insecure");
                entry.insert(RegistryConfig {
                    reg_type: RegistryType::Oci,
                    allow_insecure: true,
                    ..Default::default()
                });
            }
        }
    });

    // update allow_latest for all registries
    registry_config.iter_mut().for_each(|(url, config)| {
        if allow_latest {
            debug!(oci_registry_url = %url, "set allow_latest");
        }
        config.allow_latest = allow_latest;
    });
}

impl Host {
    const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

    const NAME_ADJECTIVES: &'static str = "
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

    const NAME_NOUNS: &'static str = "
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
    #[instrument(level = "debug", skip_all)]
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
        labels.extend(config.labels.clone().into_iter());
        let friendly_name =
            Self::generate_friendly_name().context("failed to generate friendly name")?;

        let start_evt = json!({
            "friendly_name": friendly_name,
            "labels": labels,
            "uptime_seconds": 0,
            "version": env!("CARGO_PKG_VERSION"),
        });

        let ((ctl_nats, queue), rpc_nats) = try_join!(
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
                    &config.lattice,
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
        let bucket = format!("LATTICEDATA_{}", config.lattice);
        let data = create_bucket(&ctl_jetstream, &bucket).await?;

        let config_bucket = format!("CONFIGDATA_{}", config.lattice);
        let config_data = create_bucket(&ctl_jetstream, &config_bucket).await?;

        let (queue_abort, queue_abort_reg) = AbortHandle::new_pair();
        let (heartbeat_abort, heartbeat_abort_reg) = AbortHandle::new_pair();
        let (data_watch_abort, data_watch_abort_reg) = AbortHandle::new_pair();
        let (config_data_watch_abort, config_data_watch_abort_reg) = AbortHandle::new_pair();

        let supplemental_config = if config.config_service_enabled {
            load_supplemental_config(&ctl_nats, &config.lattice, &labels).await?
        } else {
            SupplementalConfig::default()
        };

        let registry_config = RwLock::new(supplemental_config.registry_config.unwrap_or_default());
        merge_registry_config(&registry_config, config.oci_opts.clone()).await;

        let policy_manager = PolicyManager::new(
            ctl_nats.clone(),
            PolicyHostInfo {
                public_key: host_key.public_key(),
                lattice_id: config.lattice.clone(),
                labels: labels.clone(),
                cluster_issuers: cluster_issuers.clone(),
            },
            config.policy_service_config.policy_topic.clone(),
            config.policy_service_config.policy_timeout_ms,
            config.policy_service_config.policy_changes_topic.clone(),
        )
        .await?;

        let meter = global::meter_with_version(
            "wasmcloud-host",
            Some(env!("CARGO_PKG_VERSION")),
            None::<&str>,
            Some(vec![
                KeyValue::new("host.id", host_key.public_key()),
                KeyValue::new("host.version", env!("CARGO_PKG_VERSION")),
            ]),
        );
        let metrics = HostMetrics::new(
            &meter,
            host_key.public_key().clone(),
            config.lattice.clone(),
        );

        let host = Host {
            actors: RwLock::default(),
            cluster_key,
            cluster_issuers,
            event_builder,
            friendly_name,
            heartbeat: heartbeat_abort.clone(),
            ctl_topic_prefix: config.ctl_topic_prefix.clone(),
            host_key,
            labels: RwLock::new(labels),
            ctl_nats,
            rpc_nats,
            host_config: config,
            data: data.clone(),
            data_watch: data_watch_abort.clone(),
            config_data: config_data.clone(),
            config_data_watch: config_data_watch_abort.clone(),
            policy_manager,
            providers: RwLock::default(),
            registry_config,
            runtime,
            start_at,
            stop_rx,
            stop_tx,
            queue: queue_abort.clone(),
            links: RwLock::default(),
            actor_claims: Arc::default(),
            provider_claims: Arc::default(),
            config_data_cache: Arc::default(),
            metrics: Arc::new(metrics),
        };

        let host = Arc::new(host);
        let queue = spawn({
            let host = Arc::clone(&host);
            async move {
                let mut queue = Abortable::new(queue, queue_abort_reg);
                queue
                    .by_ref()
                    .for_each_concurrent(None, {
                        let host = Arc::clone(&host);
                        move |msg| {
                            let host = Arc::clone(&host);
                            async move { host.handle_ctl_message(msg).await }
                        }
                    })
                    .await;
                let deadline = { *host.stop_rx.borrow() };
                host.stop_tx.send_replace(deadline);
                if queue.is_aborted() {
                    info!("control interface queue task gracefully stopped");
                } else {
                    error!("control interface queue task unexpectedly stopped");
                }
            }
        });

        let data_watch: JoinHandle<anyhow::Result<_>> = spawn({
            let data = data.clone();
            let host = Arc::clone(&host);
            async move {
                let data_watch = data
                    .watch_all()
                    .await
                    .context("failed to watch lattice data bucket")?;
                let mut data_watch = Abortable::new(data_watch, data_watch_abort_reg);
                data_watch
                    .by_ref()
                    .for_each({
                        let host = Arc::clone(&host);
                        move |entry| {
                            let host = Arc::clone(&host);
                            async move {
                                match entry {
                                    Err(error) => {
                                        error!("failed to watch lattice data bucket: {error}");
                                    }
                                    Ok(entry) => host.process_entry(entry, true).await,
                                }
                            }
                        }
                    })
                    .await;
                let deadline = { *host.stop_rx.borrow() };
                host.stop_tx.send_replace(deadline);
                if data_watch.is_aborted() {
                    info!("data watch task gracefully stopped");
                } else {
                    error!("data watch task unexpectedly stopped");
                }
                Ok(())
            }
        });

        let config_data_watch: JoinHandle<anyhow::Result<_>> = spawn({
            let data = config_data.clone();
            let host = Arc::clone(&host);
            async move {
                let data_watch = data
                    .watch_all()
                    .await
                    .context("failed to watch config data bucket")?;
                let mut data_watch = Abortable::new(data_watch, config_data_watch_abort_reg);
                data_watch
                    .by_ref()
                    .for_each({
                        let host = Arc::clone(&host);
                        move |entry| {
                            let host = Arc::clone(&host);
                            async move {
                                match entry {
                                    Err(error) => {
                                        error!("failed to watch lattice data bucket: {error}");
                                    }
                                    Ok(entry) => host.process_config_entry(entry).await,
                                }
                            }
                        }
                    })
                    .await;
                let deadline = { *host.stop_rx.borrow() };
                host.stop_tx.send_replace(deadline);
                if data_watch.is_aborted() {
                    info!("config data watch task gracefully stopped");
                } else {
                    error!("config data watch task unexpectedly stopped");
                }
                Ok(())
            }
        });
        let heartbeat = spawn({
            let host = Arc::clone(&host);
            async move {
                let mut heartbeat = Abortable::new(heartbeat, heartbeat_abort_reg);
                heartbeat
                    .by_ref()
                    .for_each({
                        let host = Arc::clone(&host);
                        move |_| {
                            let host = Arc::clone(&host);
                            async move {
                                let heartbeat = match host.heartbeat().await {
                                    Ok(heartbeat) => heartbeat,
                                    Err(e) => {
                                        error!("failed to generate heartbeat: {e}");
                                        return;
                                    }
                                };

                                if let Err(e) =
                                    host.publish_event("host_heartbeat", heartbeat).await
                                {
                                    error!("failed to publish heartbeat: {e}");
                                }
                            }
                        }
                    })
                    .await;
                let deadline = { *host.stop_rx.borrow() };
                host.stop_tx.send_replace(deadline);
                if heartbeat.is_aborted() {
                    info!("heartbeat task gracefully stopped");
                } else {
                    error!("heartbeat task unexpectedly stopped");
                }
            }
        });

        // Process existing data without emitting events
        data.keys()
            .await
            .context("failed to read keys of lattice data bucket")?
            .map_err(|e| anyhow!(e).context("failed to read lattice data stream"))
            .try_filter_map(|key| async {
                data.entry(key)
                    .await
                    .context("failed to get entry in lattice data bucket")
            })
            .for_each(|entry| async {
                match entry {
                    Ok(entry) => host.process_entry(entry, false).await,
                    Err(err) => error!(%err, "failed to read entry from lattice data bucket"),
                }
            })
            .await;

        config_data
            .keys()
            .await
            .context("failed to read keys of config data bucket")?
            .map_err(|e| anyhow!(e).context("failed to read config data stream"))
            .try_filter_map(|key| async {
                config_data
                    .entry(key)
                    .await
                    .context("failed to get entry in config data bucket")
            })
            .for_each(|res| async {
                match res {
                    Ok(entry) => host.process_config_entry(entry).await,
                    Err(err) => error!(%err, "failed to read entry from config data bucket"),
                }
            })
            .await;

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
            config_data_watch_abort.abort();
            host.policy_manager.policy_changes.abort();
            let _ = try_join!(queue, data_watch, config_data_watch, heartbeat)
                .context("failed to await tasks")?;
            host.publish_event(
                "host_stopped",
                json!({
                    "labels": *host.labels.read().await,
                }),
            )
            .await
            .context("failed to publish stop event")?;
            // Before we exit, make sure to flush all messages or we may lose some that we've
            // thought were sent (like the host_stopped event)
            try_join!(host.ctl_nats.flush(), host.rpc_nats.flush(),)
                .context("failed to flush NATS clients")?;
            Ok(())
        }))
    }

    /// Waits for host to be stopped via lattice commands and returns the shutdown deadline on
    /// success
    ///
    /// # Errors
    ///
    /// Returns an error if internal stop channel is closed prematurely
    #[instrument(level = "debug", skip_all)]
    pub async fn stopped(&self) -> anyhow::Result<Option<Instant>> {
        self.stop_rx
            .clone()
            .changed()
            .await
            .context("failed to wait for stop")?;
        Ok(*self.stop_rx.borrow())
    }

    #[instrument(level = "debug", skip_all)]
    async fn inventory(&self) -> HostInventory {
        trace!("generating host inventory");
        let actors = self.actors.read().await;
        let actors: Vec<_> = stream::iter(actors.iter())
            .filter_map(|(id, actor)| async move {
                let name = actor
                    .claims()
                    .and_then(|claims| claims.metadata.as_ref())
                    .and_then(|metadata| metadata.name.as_ref())
                    .cloned();
                Some(ActorDescription {
                    id: id.into(),
                    image_ref: actor.image_reference.clone(),
                    annotations: Some(actor.annotations.clone().into_iter().collect()),
                    max_instances: actor.max_instances.get().try_into().unwrap_or(u32::MAX),
                    revision: actor
                        .claims()
                        .and_then(|claims| claims.metadata.as_ref())
                        .and_then(|jwt::Actor { rev, .. }| *rev)
                        .unwrap_or_default(),
                    name,
                })
            })
            .collect()
            .await;
        let providers: Vec<_> = self
            .providers
            .read()
            .await
            .iter()
            .filter_map(
                |(
                    provider_id,
                    Provider {
                        annotations,
                        claims,
                        image_ref,
                        ..
                    },
                )| {
                    let jwt::CapabilityProvider {
                        name,
                        rev: revision,
                        ..
                    } = claims.metadata.as_ref()?;
                    let annotations = Some(annotations.clone().into_iter().collect());
                    let revision = revision.unwrap_or_default();
                    Some(ProviderDescription {
                        id: provider_id.into(),
                        image_ref: Some(image_ref.clone()),
                        name: name.clone(),
                        annotations,
                        revision,
                    })
                },
            )
            .collect();
        let uptime = self.start_at.elapsed();
        HostInventory {
            actors,
            providers,
            friendly_name: self.friendly_name.clone(),
            labels: self.labels.read().await.clone(),
            uptime_human: human_friendly_uptime(uptime),
            uptime_seconds: uptime.as_secs(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            host_id: self.host_key.public_key(),
            issuer: self.cluster_key.public_key(),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn heartbeat(&self) -> anyhow::Result<serde_json::Value> {
        trace!("generating heartbeat");
        Ok(serde_json::to_value(self.inventory().await)?)
    }

    #[instrument(level = "debug", skip(self))]
    async fn publish_event(&self, name: &str, data: serde_json::Value) -> anyhow::Result<()> {
        event::publish(
            &self.event_builder,
            &self.ctl_nats,
            &self.host_config.lattice,
            name,
            data,
        )
        .await
    }

    /// Instantiate an actor
    #[allow(clippy::too_many_arguments)] // TODO: refactor into a config struct
    #[instrument(level = "debug", skip_all)]
    async fn instantiate_actor(
        &self,
        annotations: &Annotations,
        actor_ref: impl AsRef<str>,
        actor_id: impl AsRef<str>,
        max_instances: NonZeroUsize,
        component: wasmcloud_runtime::Component,
        handler: Handler,
    ) -> anyhow::Result<Arc<Actor>> {
        trace!(
            actor_ref = actor_ref.as_ref(),
            max_instances,
            "instantiating actor"
        );

        let actor_ref = actor_ref.as_ref();
        let actor_id = actor_id.as_ref();
        let component = component.clone();
        let handler = handler.clone();

        let lattice = &self.host_config.lattice;
        let prefix = format!("{lattice}.{actor_id}");
        let wrpc_client = wrpc_transport_nats::Client::new(self.rpc_nats.clone(), prefix);
        let instance = async move {
            let (calls_abort, calls_abort_reg) = AbortHandle::new_pair();
            let actor_instance = Arc::new(Actor {
                component: component.clone(),
                id: actor_id.to_string(),
                calls: calls_abort,
                handler: handler.clone(),
                annotations: annotations.clone(),
                max_instances,
                valid_issuers: self.cluster_issuers.clone(),
                policy_manager: Arc::clone(&self.policy_manager),
                image_reference: actor_ref.to_string(),
                actor_claims: Arc::clone(&self.actor_claims),
                metrics: Arc::clone(&self.metrics),
            });

            let mut component_export_handlers = Vec::new();
            let exports = component.exports();
            for (instance, name_and_func) in exports.iter() {
                for (name, function) in name_and_func {
                    if let wrpc_types::DynamicFunction::Static { params, .. } = function {
                        let instance = instance.clone();
                        let name = name.clone();
                        // TODO(#1220): In order to implement invocation signing and response verification, we can override the
                        // wrpc_transport::Invocation and wrpc_transport::Client trait in order to wrap the invocation with necessary
                        // logic to verify the incoming invocations and sign the outgoing responses.
                        trace!(instance, name, "serving wrpc function export");
                        let export_handler = wrpc_client
                            .serve_dynamic(&instance, &name, params.clone())
                            .await
                            .expect("should be able to serve function export")
                            // map the stream to include the instance and name to call for each invocation
                            .map(move |invocation| (instance.clone(), name.clone(), invocation));
                        component_export_handlers.push(export_handler);
                    }
                }
            }

            let all_handlers = futures::stream::select_all::select_all(component_export_handlers);
            let limit = max_instances.get();
            let actor_instance = Arc::clone(&actor_instance);
            let _calls = spawn({
                let actor_instance = Arc::clone(&actor_instance);
                Abortable::new(all_handlers, calls_abort_reg).for_each_concurrent(
                    limit,
                    move |(instance, name, invocation)| {
                        let actor_instance = Arc::clone(&actor_instance);
                        async move {
                            let AcceptedInvocation {
                                context,
                                params,
                                result_subject,
                                error_subject,
                                transmitter,
                            } = match invocation {
                                Ok(invocation) => invocation,
                                Err(err) => {
                                    error!(?err, "failed to accept invocation");
                                    return;
                                }
                            };
                            // TODO(#1568): Propagate the trace parent from context (headers)
                            // Linked PR above is when the functionality was removed.
                            _ = context;
                            let actor_instance = Arc::clone(&actor_instance);
                            if let Err(err) = actor_instance
                                .handle_wrpc_call(
                                    &instance,
                                    &name,
                                    params,
                                    result_subject,
                                    &transmitter,
                                )
                                .await
                            {
                                error!(?err, "failed to handle invocation");
                                if let Err(err) = transmitter
                                    .transmit_static(error_subject, format!("{err:#}"))
                                    .await
                                {
                                    error!(?err, "failed to transmit error to invoker");
                                }
                            }
                        }
                    },
                )
            });
            anyhow::Result::<_>::Ok(actor_instance)
        }
        .await
        .context("failed to instantiate actor")?;

        Ok(instance)
    }

    #[instrument(level = "debug", skip_all)]
    async fn start_actor<'a>(
        &self,
        entry: hash_map::VacantEntry<'a, String, Arc<Actor>>,
        component: wasmcloud_runtime::Component,
        actor_ref: String,
        actor_id: String,
        max_instances: NonZeroUsize,
        annotations: impl Into<Annotations>,
    ) -> anyhow::Result<&'a mut Arc<Actor>> {
        debug!(actor_ref, ?max_instances, "starting new actor");

        let annotations = annotations.into();
        let claims = component.claims().context("claims missing")?;
        let component_spec = if let Ok(spec) = self.get_component_spec(&actor_id).await {
            if spec.url != actor_ref {
                bail!(
                    "component spec URL does not match actor reference: {} != {}",
                    spec.url,
                    actor_ref
                );
            }
            spec
        } else {
            let spec = ComponentSpecification::new(&actor_ref);
            self.store_component_spec(&actor_id, &spec).await?;
            spec
        };
        self.store_claims(Claims::Actor(claims.clone()))
            .await
            .context("failed to store claims")?;

        let origin = WasmCloudEntity {
            public_key: claims.subject.clone(),
            ..Default::default()
        };
        let polyfilled_imports = component.polyfilled_imports().clone();
        // Map the imports to pull out the result types of the functions for lookup when invoking them
        let imports = polyfilled_imports
            .iter()
            .map(|(instance, funcs)| {
                (
                    instance.clone(),
                    funcs
                        .iter()
                        .filter_map(|(name, func)| {
                            match func {
                                DynamicFunction::Static { results, .. } => {
                                    Some((name.clone(), results.clone()))
                                }
                                // We do not support method imports (on resources) at this time.
                                DynamicFunction::Method { .. } => None,
                            }
                        })
                        .collect::<HashMap<_, _>>(),
                )
            })
            .collect::<HashMap<_, _>>();

        let handler = Handler {
            nats: self.rpc_nats.clone(),
            config_data: Arc::clone(&self.config_data_cache),
            lattice: self.host_config.lattice.clone(),
            origin,
            cluster_key: Arc::clone(&self.cluster_key),
            claims: claims.clone(),
            interface_link_name: Arc::new(RwLock::new("default".to_string())),
            interface_links: Arc::new(RwLock::new(component_spec.links)),
            host_key: Arc::clone(&self.host_key),
            polyfilled_imports: imports,
        };

        let actor = self
            .instantiate_actor(
                &annotations,
                &actor_ref,
                &actor_id,
                max_instances,
                component.clone(),
                handler.clone(),
            )
            .await
            .context("failed to instantiate actor")?;

        info!(actor_ref, "actor started");
        self.publish_event(
            "actor_scaled",
            event::actor_scaled(
                claims,
                &annotations,
                &self.host_key.public_key(),
                max_instances,
                &actor_ref,
            ),
        )
        .await?;

        Ok(entry.insert(actor))
    }

    #[instrument(level = "debug", skip_all)]
    async fn stop_actor(&self, actor: &Actor, host_id: &str) -> anyhow::Result<()> {
        trace!(actor_id = %actor.id, "stopping actor");

        actor.calls.abort();

        let claims = actor.claims().context("claims missing")?;
        self.publish_event(
            "actor_scaled",
            event::actor_scaled(
                claims,
                &actor.annotations,
                host_id,
                0_usize,
                &actor.image_reference,
            ),
        )
        .await?;

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_auction_actor(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<Option<CtlResponse<ActorAuctionAck>>> {
        let ActorAuctionRequest {
            actor_ref,
            actor_id,
            constraints,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor auction command")?;

        info!(
            actor_ref,
            actor_id,
            ?constraints,
            "handling auction for actor"
        );

        let host_labels = self.labels.read().await;
        let constraints_satisfied = constraints
            .iter()
            .all(|(k, v)| host_labels.get(k).map(|hv| hv == v).unwrap_or(false));
        let actor_id_running = self.actors.read().await.contains_key(&actor_id);

        // This host can run the actor if all constraints are satisfied and the actor is not already running
        if constraints_satisfied && !actor_id_running {
            Ok(Some(CtlResponse::ok(ActorAuctionAck {
                actor_ref,
                actor_id,
                constraints,
                host_id: self.host_key.public_key(),
            })))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_auction_provider(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<Option<CtlResponse<ProviderAuctionAck>>> {
        let ProviderAuctionRequest {
            provider_ref,
            provider_id,
            constraints,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider auction command")?;

        info!(
            provider_ref,
            provider_id,
            ?constraints,
            "handling auction for provider"
        );

        let host_labels = self.labels.read().await;
        let constraints_satisfied = constraints
            .iter()
            .all(|(k, v)| host_labels.get(k).map(|hv| hv == v).unwrap_or(false));
        let providers = self.providers.read().await;
        let provider_running = providers.contains_key(&provider_id);
        if constraints_satisfied && !provider_running {
            Ok(Some(CtlResponse::ok(ProviderAuctionAck {
                provider_ref,
                provider_id,
                constraints,
                host_id: self.host_key.public_key(),
            })))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn fetch_actor(&self, actor_ref: &str) -> anyhow::Result<wasmcloud_runtime::Component> {
        let registry_config = self.registry_config.read().await;
        let actor = fetch_actor(
            actor_ref,
            self.host_config.allow_file_load,
            &registry_config,
        )
        .await
        .context("failed to fetch actor")?;
        let actor = wasmcloud_runtime::Component::new(&self.runtime, actor)
            .context("failed to initialize actor")?;
        Ok(actor)
    }

    #[instrument(level = "trace", skip_all)]
    async fn store_actor_claims(&self, claims: jwt::Claims<jwt::Actor>) -> anyhow::Result<()> {
        let mut actor_claims = self.actor_claims.write().await;
        actor_claims.insert(claims.subject.clone(), claims);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_stop_host(
        &self,
        payload: impl AsRef<[u8]>,
        _host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let StopHostCommand { timeout, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize stop command")?;

        info!(?timeout, "handling stop host");

        self.heartbeat.abort();
        self.data_watch.abort();
        self.config_data_watch.abort();
        self.queue.abort();
        self.policy_manager.policy_changes.abort();
        let deadline =
            timeout.and_then(|timeout| Instant::now().checked_add(Duration::from_millis(timeout)));
        self.stop_tx.send_replace(deadline);
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_scale_actor(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let ScaleActorCommand {
            actor_ref,
            actor_id,
            annotations,
            max_instances,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor scale command")?;

        debug!(actor_ref, max_instances, actor_id, "handling scale actor");

        let host_id = host_id.to_string();
        let annotations: Annotations = annotations.unwrap_or_default().into_iter().collect();
        spawn(async move {
            if let Err(e) = self
                .handle_scale_actor_task(
                    &actor_ref,
                    &actor_id,
                    &host_id,
                    max_instances,
                    annotations,
                )
                .await
            {
                error!(%actor_ref, %actor_id, err = ?e, "failed to scale actor");
            }
        });
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    /// Handles scaling an actor to a supplied number of `max` concurrently executing instances.
    /// Supplying `0` will result in stopping that actor instance.
    async fn handle_scale_actor_task(
        &self,
        actor_ref: &str,
        actor_id: &str,
        host_id: &str,
        max_instances: u32,
        annotations: Annotations,
    ) -> anyhow::Result<()> {
        trace!(actor_ref, max_instances, "scale actor task");

        let actor = self.fetch_actor(actor_ref).await?;
        let claims = actor.claims().context("claims missing")?;
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
                "Policy denied request to scale actor `{}`: `{:?}`",
                resp.request_id,
                resp.message
            )
        };

        let actor_ref = actor_ref.to_string();
        match (
            self.actors.write().await.entry(actor_id.to_string()),
            NonZeroUsize::new(max_instances as usize),
        ) {
            // No actor is running and we requested to scale to zero, noop
            (hash_map::Entry::Vacant(_), None) => {}
            // No actor is running and we requested to scale to some amount, start with specified max
            (hash_map::Entry::Vacant(entry), Some(max)) => {
                if let Err(e) = self
                    .start_actor(
                        entry,
                        actor.clone(),
                        actor_ref.clone(),
                        actor_id.to_string(),
                        max,
                        annotations.clone(),
                    )
                    .await
                {
                    self.publish_event(
                        "actor_scale_failed",
                        event::actor_scale_failed(
                            claims,
                            &annotations,
                            host_id,
                            &actor_ref,
                            max,
                            &e,
                        ),
                    )
                    .await?;
                    return Err(e);
                }
            }
            // Actor is running and we requested to scale to zero instances, stop actor
            (hash_map::Entry::Occupied(entry), None) => {
                let actor = entry.remove();
                if let Err(err) = self
                    .stop_actor(&actor, host_id)
                    .await
                    .context("failed to stop actor in response to scale to zero")
                {
                    self.publish_event(
                        "actor_scale_failed",
                        event::actor_scale_failed(
                            claims,
                            &actor.annotations,
                            host_id,
                            actor_ref,
                            actor.max_instances,
                            &err,
                        ),
                    )
                    .await?;
                    return Err(err);
                };

                info!(actor_ref, "actor stopped");
                self.publish_event(
                    "actor_scaled",
                    event::actor_scaled(
                        claims,
                        &actor.annotations,
                        host_id,
                        0_usize,
                        &actor.image_reference,
                    ),
                )
                .await?;
            }
            // Actor is running and we requested to scale to some amount or unbounded, scale actor
            (hash_map::Entry::Occupied(mut entry), Some(max)) => {
                let actor = entry.get_mut();
                // Modify scale only if the requested max differs from the current max
                if actor.max_instances != max {
                    let instance = self
                        .instantiate_actor(
                            &annotations,
                            &actor_ref,
                            &actor.id,
                            max,
                            actor.component.clone(),
                            actor.handler.clone(),
                        )
                        .await
                        .context("failed to instantiate actor")?;
                    let publish_result = match actor.max_instances.cmp(&max) {
                        std::cmp::Ordering::Less | std::cmp::Ordering::Greater => {
                            self.publish_event(
                                "actor_scaled",
                                event::actor_scaled(
                                    claims,
                                    &actor.annotations,
                                    host_id,
                                    max,
                                    &actor.image_reference,
                                ),
                            )
                            .await
                        }
                        std::cmp::Ordering::Equal => Ok(()),
                    };
                    let actor = entry.insert(instance);
                    self.stop_actor(&actor, host_id)
                        .await
                        .context("failed to stop actor after scaling")?;

                    info!(actor_ref, ?max, "actor scaled");

                    // Wait to unwrap the event publish result until after we've processed the instances
                    publish_result?;
                }
            }
        }
        Ok(())
    }

    // TODO(#1548): With actor IDs, new actor references, configuration, etc, we're going to need to do some
    // design thinking around how update actor should work. Should it be limited to a single host or latticewide?
    // Should it also update configuration, or is that separate? Should scaling be done via an update?
    #[instrument(level = "debug", skip_all)]
    async fn handle_update_actor(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let UpdateActorCommand {
            actor_id,
            annotations,
            new_actor_ref,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor update command")?;

        debug!(
            actor_id,
            new_actor_ref,
            ?annotations,
            "handling update actor"
        );

        let actors = self.actors.read().await;
        let actor = actors.get(&actor_id).context("actor not found")?;
        let annotations = annotations.unwrap_or_default().into_iter().collect();

        let new_actor = self.fetch_actor(&new_actor_ref).await?;
        let new_claims = new_actor
            .claims()
            .context("claims missing from new actor")?;
        self.store_claims(Claims::Actor(new_claims.clone()))
            .await
            .context("failed to store claims")?;

        let max = actor.max_instances;
        let Ok(new_actor) = self
            .instantiate_actor(
                &annotations,
                &new_actor_ref,
                &actor_id,
                max,
                new_actor.clone(),
                actor.handler.clone(),
            )
            .await
        else {
            bail!("failed to instantiate actor from new reference");
        };

        info!(%new_actor_ref, "actor updated");
        self.publish_event(
            "actor_scaled",
            event::actor_scaled(new_claims, &actor.annotations, host_id, max, &new_actor_ref),
        )
        .await?;

        let old_claims = actor
            .claims()
            .context("claims missing from running actor")?;

        // TODO(#1548): If this errors, we need to rollback
        self.stop_actor(actor, host_id)
            .await
            .context("failed to stop old actor")?;
        self.publish_event(
            "actor_scaled",
            event::actor_scaled(
                old_claims,
                &actor.annotations,
                host_id,
                0_usize,
                &actor.image_reference,
            ),
        )
        .await?;

        self.actors.write().await.insert(actor_id, new_actor);

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_start_provider_task(
        &self,
        configuration: Option<String>,
        provider_id: &str,
        provider_ref: &str,
        annotations: HashMap<String, String>,
        host_id: &str,
    ) -> anyhow::Result<()> {
        trace!(provider_ref, provider_id, "start provider task");

        let registry_config = self.registry_config.read().await;
        let (path, claims) = crate::fetch_provider(
            provider_ref,
            // TODO: we cache based on link name, why
            provider_id,
            self.host_config.allow_file_load,
            &registry_config,
        )
        .await
        .context("failed to fetch provider")?;

        let target = PolicyRequestTarget::from(claims.clone());
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

        if let Ok(spec) = self.get_component_spec(provider_id).await {
            if spec.url != provider_ref {
                bail!(
                    "component specification URL does not match provider reference: {} != {}",
                    spec.url,
                    provider_ref
                );
            }
            spec
        } else {
            let spec = ComponentSpecification::new(provider_ref);
            self.store_component_spec(&provider_id, &spec).await?;
            spec
        };
        self.store_claims(Claims::Provider(claims.clone()))
            .await
            .context("failed to store claims")?;

        let annotations: Annotations = annotations.into_iter().collect();
        let mut providers = self.providers.write().await;
        if let hash_map::Entry::Vacant(entry) = providers.entry(provider_id.into()) {
            let invocation_seed = self
                .cluster_key
                .seed()
                .context("cluster key seed missing")?;
            let lattice_rpc_user_seed = self
                .host_config
                .rpc_key
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
                lattice_rpc_prefix: self.host_config.lattice.clone(),
                link_name: "default".to_string(),
                lattice_rpc_user_jwt: self.host_config.rpc_jwt.clone().unwrap_or_default(),
                lattice_rpc_user_seed: lattice_rpc_user_seed.unwrap_or_default(),
                lattice_rpc_url: self.host_config.rpc_nats_url.to_string(),
                env_values: vec![],
                instance_id: Uuid::new_v4().to_string(),
                provider_key: provider_id.to_string(),
                // TODO(#1548): Providers should receive [wasmcloud_control_interface::InterfaceLinkDefinition]s
                link_definitions: vec![],
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

            trace!("spawn provider process");

            let mut child_cmd = process::Command::new(&path);
            // Prevent the provider from inheriting the host's environment, with the exception of
            // the following variables we manually add back
            child_cmd.env_clear();

            // TODO: remove these OTEL vars once all providers are updated to use the new SDK
            child_cmd
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
                );

            if cfg!(windows) {
                // Proxy SYSTEMROOT to providers. Without this, providers on Windows won't be able to start
                child_cmd.env(
                    "SYSTEMROOT",
                    env::var("SYSTEMROOT")
                        .context("SYSTEMROOT is not set. Providers cannot be started")?,
                );
            }

            // Proxy RUST_LOG to (Rust) providers, so they can use the same module-level directives
            if let Ok(rust_log) = env::var("RUST_LOG") {
                let _ = child_cmd.env("RUST_LOG", rust_log);
            }

            let mut child = child_cmd
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
            let rpc_nats = self.rpc_nats.clone();
            let ctl_nats = self.ctl_nats.clone();
            let event_builder = self.event_builder.clone();
            // NOTE: health_ prefix here is to allow us to move the variables into the closure
            let health_lattice = self.host_config.lattice.clone();
            let health_provider_id = claims.subject.to_string();
            let health_contract_id = claims.metadata.clone().map(|m| m.capid).unwrap_or_default();
            let child = spawn(async move {
                // Check the health of the provider every 30 seconds
                let mut health_check = tokio::time::interval(Duration::from_secs(30));
                let mut previous_healthy = false;
                // Allow the provider 5 seconds to initialize
                health_check.reset_after(Duration::from_secs(5));
                let health_topic =
                    format!("wasmbus.rpc.{health_lattice}.{health_provider_id}.default.health");
                // TODO: Refactor this logic to simplify nesting
                loop {
                    select! {
                        _ = health_check.tick() => {
                            trace!(provider_id=health_provider_id, "performing provider health check");
                            let request = async_nats::Request::new()
                                .payload(Bytes::new())
                                .headers(injector_to_headers(&TraceContextInjector::default_with_span()));
                            if let Ok(async_nats::Message { payload, ..}) = rpc_nats.send_request(
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
                                                &health_lattice,
                                                "health_check_passed",
                                                event::provider_health_check(
                                                    &health_provider_id,
                                                    "default",
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
                                                &health_lattice,
                                                "health_check_failed",
                                                event::provider_health_check(
                                                    &health_provider_id,
                                                    "default",
                                                    &health_contract_id,
                                                )
                                            ).await {
                                                warn!(?e, "failed to publish provider health check failed event");
                                            }
                                        }
                                        // If the provider health status didn't change, we simply publish a health check status event
                                        (Ok(_), _) => {
                                            if let Err(e) = event::publish(
                                                &event_builder,
                                                &ctl_nats,
                                                &health_lattice,
                                                "health_check_status",
                                                event::provider_health_check(
                                                    &health_provider_id,
                                                   "default",
                                                    &health_contract_id,
                                                )
                                            ).await {
                                                warn!(?e, "failed to publish provider health check status event");
                                            }
                                        },
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
            info!(provider_ref, provider_id, "provider started");
            self.publish_event(
                "provider_started",
                event::provider_started(&claims, &annotations, host_id, provider_ref, provider_id),
            )
            .await?;
            entry.insert(Provider {
                child,
                annotations,
                claims,
                image_ref: provider_ref.to_string(),
            });
        } else {
            bail!("provider is already running with that ID")
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_start_provider(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let StartProviderCommand {
            configuration,
            provider_id,
            provider_ref,
            annotations,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider start command")?;

        if self.providers.read().await.contains_key(&provider_id) {
            return Ok(CtlResponse::error(
                "provider with that ID is already running",
            ));
        }

        info!(provider_ref, provider_id, "handling start provider"); // Log at info since starting providers can take a while

        let host_id = host_id.to_string();
        spawn(async move {
            if let Err(err) = self
                .handle_start_provider_task(
                    configuration,
                    &provider_id,
                    &provider_ref,
                    annotations.unwrap_or_default(),
                    &host_id,
                )
                .await
            {
                error!(provider_ref, provider_id, ?err, "failed to start provider");
                if let Err(err) = self
                    .publish_event(
                        "provider_start_failed",
                        event::provider_start_failed(provider_ref, provider_id, &err),
                    )
                    .await
                {
                    error!(?err, "failed to publish provider_start_failed event");
                }
            }
        });
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_stop_provider(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let StopProviderCommand { provider_id, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider stop command")?;

        debug!(provider_id, "handling stop provider");

        let mut providers = self.providers.write().await;
        let hash_map::Entry::Occupied(entry) = providers.entry(provider_id.clone()) else {
            warn!(
                provider_id,
                "received request to stop provider that is not running"
            );
            return Ok(CtlResponse::error("provider with that ID is not running"));
        };
        let Provider {
            child,
            annotations,
            claims,
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
            .rpc_nats
            .send_request(
                format!(
                    "wasmbus.rpc.{}.{provider_id}.default.shutdown",
                    self.host_config.lattice
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
        info!(provider_id, "provider stopped");
        self.publish_event(
            "provider_stopped",
            event::provider_stopped(&claims, &annotations, host_id, provider_id, "stop"),
        )
        .await?;
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_inventory(&self) -> anyhow::Result<CtlResponse<HostInventory>> {
        trace!("handling inventory");
        let inventory = self.inventory().await;
        Ok(CtlResponse::ok(inventory))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_claims(&self) -> anyhow::Result<CtlResponse<GetClaimsResponse>> {
        trace!("handling claims");

        let (actor_claims, provider_claims) =
            join!(self.actor_claims.read(), self.provider_claims.read());
        let actor_claims = actor_claims.values().cloned().map(Claims::Actor);
        let provider_claims = provider_claims.values().cloned().map(Claims::Provider);
        let claims: Vec<StoredClaims> = actor_claims
            .chain(provider_claims)
            .flat_map(TryFrom::try_from)
            .collect();

        Ok(CtlResponse::ok(GetClaimsResponse {
            claims: claims.into_iter().map(std::convert::Into::into).collect(),
        }))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_links(&self) -> anyhow::Result<Vec<u8>> {
        debug!("handling links");

        let links = self.links.read().await;
        let links: Vec<&InterfaceLinkDefinition> = links.values().flatten().collect();
        let res =
            serde_json::to_vec(&CtlResponse::ok(links)).context("failed to serialize response")?;
        Ok(res)
    }

    #[instrument(level = "trace", skip(self))]
    async fn handle_config_get(&self, entity_id: &str) -> anyhow::Result<Vec<u8>> {
        trace!(%entity_id, "handling all config");
        self.config_data_cache
            .read()
            .await
            .get(entity_id)
            .map_or_else(
                || {
                    serde_json::to_vec(&CtlResponse::error("config not found"))
                        .map_err(anyhow::Error::from)
                },
                |data| serde_json::to_vec(&CtlResponse::ok(data)).map_err(anyhow::Error::from),
            )
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_label_put(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<CtlResponse<()>> {
        let HostLabel { key, value } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize put label request")?;
        let mut labels = self.labels.write().await;
        match labels.entry(key) {
            Entry::Occupied(mut entry) => {
                info!(key = entry.key(), value, "updated label");
                entry.insert(value);
            }
            Entry::Vacant(entry) => {
                info!(key = entry.key(), value, "set label");
                entry.insert(value);
            }
        }
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_label_del(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<CtlResponse<()>> {
        let HostLabel { key, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize delete label request")?;
        let mut labels = self.labels.write().await;
        if labels.remove(&key).is_some() {
            info!(key, "removed label");
        } else {
            warn!(key, "could not remove unset label");
        }
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_interface_link_put(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let payload = payload.as_ref();
        let interface_link_definition: InterfaceLinkDefinition = serde_json::from_slice(payload)
            .context("failed to deserialize wrpc link definition")?;
        let InterfaceLinkDefinition {
            source_id,
            target,
            wit_namespace,
            wit_package,
            interfaces,
            name,
            source_config: _,
            target_config: _,
        } = interface_link_definition.clone();

        let actors = self.actors.read().await;

        // TODO(#1548): When providers can handle interface links, don't just assume actors for links.
        let Ok(actor) = actors.get(&source_id).context("actor not found") else {
            tracing::error!(source_id, "no actor found for the unique id");
            return Ok(CtlResponse::error("no actor found for that ID"));
        };

        let ns_and_package = format!("{}:{}", wit_namespace, wit_package);

        debug!(
            source_id,
            target, ns_and_package, name, "handling put wrpc link definition"
        );

        // Write link for each interface in the package
        actor
            .handler
            .interface_links
            .write()
            .await
            .entry(name.clone())
            .and_modify(|link_for_name| {
                link_for_name
                    .entry(ns_and_package.clone())
                    .and_modify(|package| {
                        for interface in &interfaces {
                            package.insert(interface.clone(), target.clone());
                        }
                    })
                    .or_insert({
                        interfaces
                            .iter()
                            .map(|interface| (interface.clone(), target.clone()))
                            .collect::<HashMap<String, String>>()
                    });
            })
            .or_insert({
                let interfaces_map = interfaces
                    .iter()
                    .map(|interface| (interface.clone(), target.clone()))
                    .collect::<HashMap<String, String>>();
                HashMap::from_iter([(ns_and_package.clone(), interfaces_map)])
            });

        let spec = actor.component_specification().await;
        self.store_component_spec(&source_id, &spec).await?;

        let set_event = event::linkdef_set(&interface_link_definition);

        // Insert link into host map
        self.links
            .write()
            .await
            .entry(source_id.clone())
            .and_modify(|links| {
                links.replace(interface_link_definition.clone());
            })
            .or_insert(HashSet::from_iter([interface_link_definition]));

        self.publish_event("linkdef_set", set_event).await?;
        // TODO(#1548): When providers can handle interface links, tell them to set the link.
        // Alternatively, send them configuration cc @thomastaylor312
        // self.rpc_nats
        // .publish_with_headers(
        //     format!("wasmbus.rpc.{lattice}.{provider_id}.{link_name}.linkdefs.set",),
        //     injector_to_headers(&TraceContextInjector::default_with_span()),
        //     msgp.into(),
        // )
        // .await
        // .context("failed to publish link definition set")?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    /// Remove an interface link on a source component for a specific package
    async fn handle_interface_link_del(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let payload = payload.as_ref();
        let DeleteInterfaceLinkDefinitionRequest {
            source_id,
            wit_namespace,
            wit_package,
            name,
        } = serde_json::from_slice(payload)
            .context("failed to deserialize wrpc link definition")?;

        let actors = self.actors.read().await;

        let Ok(actor) = actors.get(&source_id).context("actor not found") else {
            return Ok(CtlResponse::success());
        };

        let ns_and_package = format!("{}:{}", wit_namespace, wit_package);

        debug!(
            source_id,
            ns_and_package, name, "handling del wrpc link definition"
        );

        // Remove the interface links for the given link name and package
        actor
            .handler
            .interface_links
            .write()
            .await
            .entry(name.clone())
            .and_modify(|link_for_name| {
                link_for_name.remove(&ns_and_package);
            });

        let spec = actor.component_specification().await;
        self.store_component_spec(&source_id, &spec).await?;

        // Remove link from host map
        self.links
            .write()
            .await
            .entry(source_id.clone())
            .and_modify(|links| {
                // Retain links that don't match the link we're removing
                links.retain(|interface_link_definition| {
                    interface_link_definition.wit_namespace != wit_namespace
                        || interface_link_definition.wit_package != wit_package
                        || interface_link_definition.name != name
                });
            });

        self.publish_event(
            "linkdef_deleted",
            event::linkdef_deleted(source_id, name, wit_namespace, wit_package),
        )
        .await?;
        // TODO(#1548): When providers can handle interface links, tell them to set the link.
        // Alternatively, send them configuration cc @thomastaylor312
        // self.rpc_nats
        // .publish_with_headers(
        //     format!("wasmbus.rpc.{lattice}.{provider_id}.{link_name}.linkdefs.del",),
        //     injector_to_headers(&TraceContextInjector::default_with_span()),
        //     msgp.into(),
        // )
        // .await
        // .context("failed to publish link definition deletion")?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_registries_put(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let registry_creds: HashMap<String, RegistryCredential> =
            serde_json::from_slice(payload.as_ref())
                .context("failed to deserialize registries put command")?;

        info!(
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

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all, fields(%config_name))]
    async fn handle_config_put(
        &self,
        config_name: &str,
        data: Bytes,
    ) -> anyhow::Result<CtlResponse<()>> {
        debug!("handle config entry put");
        // Validate that the data is of the proper type by deserialing it
        serde_json::from_slice::<HashMap<String, String>>(&data)
            .context("Config data should be a map of string -> string")?;
        self.config_data
            .put(config_name, data)
            .await
            .context("Unable to store config data")?;
        // We don't write it into the cached data and instead let the caching thread handle it as we
        // won't need it immediately.
        self.publish_event("config_set", event::config_set(config_name))
            .await?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all, fields(%config_name))]
    async fn handle_config_delete(&self, config_name: &str) -> anyhow::Result<CtlResponse<()>> {
        debug!("handle config entry deletion");

        self.config_data
            .purge(config_name)
            .await
            .context("Unable to delete config data")?;

        self.publish_event("config_deleted", event::config_deleted(config_name))
            .await?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_ping_hosts(
        &self,
        _payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<wasmcloud_control_interface::Host>> {
        trace!("replying to ping");
        let uptime = self.start_at.elapsed();
        let cluster_issuers = self.cluster_issuers.clone().join(",");

        Ok(CtlResponse::ok(wasmcloud_control_interface::Host {
            id: self.host_key.public_key(),
            labels: self.labels.read().await.clone(),
            friendly_name: self.friendly_name.clone(),
            uptime_seconds: uptime.as_secs(),
            uptime_human: Some(human_friendly_uptime(uptime)),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            cluster_issuers: Some(cluster_issuers),
            js_domain: self.host_config.js_domain.clone(),
            ctl_host: Some(self.host_config.ctl_nats_url.to_string()),
            rpc_host: Some(self.host_config.rpc_nats_url.to_string()),
            lattice: self.host_config.lattice.clone(),
        }))
    }

    #[instrument(level = "trace", skip_all, fields(subject = %message.subject))]
    async fn handle_ctl_message(self: Arc<Self>, message: async_nats::Message) {
        // NOTE: if log level is not `trace`, this won't have an effect, since the current span is
        // disabled. In most cases that's fine, since we aren't aware of any control interface
        // requests including a trace context
        opentelemetry_nats::attach_span_context(&message);
        // Skip the topic prefix and then the lattice prefix
        // e.g. `wasmbus.ctl.{prefix}`
        let subject = message.subject;
        let mut parts = subject
            .trim()
            .trim_start_matches(&self.ctl_topic_prefix)
            .trim_start_matches('.')
            .split('.')
            .skip(1);
        trace!(%subject, "handling control interface request");

        // This response is a wrapped Result<Option<Result<Vec<u8>>>> for a good reason.
        // The outer Result is for reporting protocol errors in handling the request, e.g. failing to
        //    deserialize the request payload.
        // The Option is for the case where the request is handled successfully, but the handler
        //    doesn't want to send a response back to the client, like with an auction.
        // The inner Result is purely for the success or failure of serializing the [CtlResponse], which
        //    should never fail but it's a result we must handle.
        // And finally, the Vec<u8> is the serialized [CtlResponse] that we'll send back to the client
        let ctl_response = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            // Actor commands
            (Some("actor"), Some("auction"), None, None) => self
                .handle_auction_actor(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("actor"), Some("scale"), Some(host_id), None) => Arc::clone(&self)
                .handle_scale_actor(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("actor"), Some("update"), Some(host_id), None) => self
                .handle_update_actor(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Provider commands
            (Some("provider"), Some("auction"), None, None) => self
                .handle_auction_provider(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("provider"), Some("start"), Some(host_id), None) => Arc::clone(&self)
                .handle_start_provider(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("provider"), Some("stop"), Some(host_id), None) => self
                .handle_stop_provider(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Host commands
            (Some("host"), Some("get"), Some(_host_id), None) => self
                .handle_inventory()
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("host"), Some("ping"), None, None) => self
                .handle_ping_hosts(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("host"), Some("stop"), Some(host_id), None) => self
                .handle_stop_host(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Claims commands
            (Some("claims"), Some("get"), None, None) => self
                .handle_claims()
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Link commands
            (Some("link"), Some("del"), None, None) => self
                .handle_interface_link_del(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("link"), Some("get"), None, None) => {
                // Explicitly returning a Vec<u8> for non-cloning efficiency within handle_links
                self.handle_links().await.map(|bytes| Some(Ok(bytes)))
            }
            (Some("link"), Some("put"), None, None) => self
                .handle_interface_link_put(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Label commands
            (Some("label"), Some("del"), Some(_host_id), None) => self
                .handle_label_del(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("label"), Some("put"), Some(_host_id), None) => self
                .handle_label_put(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Registry commands
            (Some("registry"), Some("put"), None, None) => self
                .handle_registries_put(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Config commands
            (Some("config"), Some("get"), Some(config_name), None) => self
                .handle_config_get(config_name)
                .await
                .map(|bytes| Some(Ok(bytes))),
            (Some("config"), Some("put"), Some(config_name), None) => self
                .handle_config_put(config_name, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("config"), Some("del"), Some(config_name), None) => self
                .handle_config_delete(config_name)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Topic fallback
            _ => {
                warn!(%subject, "received control interface request on unsupported subject");
                Ok(serialize_ctl_response(Some(CtlResponse::error(
                    "unsupported subject",
                ))))
            }
        };

        if let Err(err) = &ctl_response {
            error!(%subject, ?err, "failed to handle control interface request");
        } else {
            trace!(%subject, "handled control interface request");
        }

        if let Some(reply) = message.reply {
            let headers = injector_to_headers(&TraceContextInjector::default_with_span());

            let payload = match ctl_response {
                Ok(Some(Ok(payload))) => Some(payload.into()),
                // No response from the host (e.g. auctioning provider)
                Ok(None) => None,
                Err(e) => Some(
                    serde_json::to_vec(&CtlResponse::error(&e.to_string()))
                        .context("failed to encode control interface response")
                        // This should never fail to serialize, but the fallback ensures that we send
                        // something back to the client even if we somehow fail.
                        .unwrap_or_else(|_| format!(r#"{{"success":false,"error":"{e}"}}"#).into())
                        .into(),
                ),
                // This would only occur if we failed to serialize a valid CtlResponse. This is
                // programmer error.
                Ok(Some(Err(e))) => Some(
                    serde_json::to_vec(&CtlResponse::error(&e.to_string()))
                        .context("failed to encode control interface response")
                        .unwrap_or_else(|_| format!(r#"{{"success":false,"error":"{e}"}}"#).into())
                        .into(),
                ),
            };

            if let Some(payload) = payload {
                if let Err(err) = self
                    .ctl_nats
                    .publish_with_headers(reply.clone(), headers, payload)
                    .err_into::<anyhow::Error>()
                    .and_then(|()| self.ctl_nats.flush().err_into::<anyhow::Error>())
                    .await
                {
                    error!(%subject, ?err, "failed to publish reply to control interface request");
                }
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn get_component_spec(&self, id: &str) -> anyhow::Result<ComponentSpecification> {
        let key = format!("COMPONENT_{id}");
        let spec = self
            .data
            .get(key)
            .await
            .context("failed to get component spec")?
            .map(|spec_bytes| serde_json::from_slice(&spec_bytes))
            .ok_or_else(|| anyhow!("component spec not found"))??;
        Ok(spec)
    }

    #[instrument(level = "debug", skip_all)]
    async fn store_component_spec(
        &self,
        id: impl AsRef<str>,
        spec: &ComponentSpecification,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        let key = format!("COMPONENT_{id}");
        let bytes = serde_json::to_vec(spec)
            .context("failed to serialize component spec")?
            .into();
        self.data
            .put(key, bytes)
            .await
            .context("failed to put component spec")?;
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
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
        let subject = match &claims {
            StoredClaims::Actor(claims) => &claims.subject,
            StoredClaims::Provider(claims) => &claims.subject,
        };
        let key = format!("CLAIMS_{subject}");
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

    #[instrument(level = "debug", skip_all)]
    async fn process_component_spec_put(
        &self,
        id: impl AsRef<str>,
        value: impl AsRef<[u8]>,
        _publish: bool,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        debug!(id, "process component spec put");

        let spec: ComponentSpecification = serde_json::from_slice(value.as_ref())
            .context("failed to deserialize component specification")?;
        if let Some(actor) = self.actors.write().await.get(id) {
            // Update links
            *actor.handler.interface_links.write().await = spec.links;
            // NOTE(brooksmtownsend): We can consider updating the actor if the image URL changes
        };

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn process_component_spec_delete(
        &self,
        id: impl AsRef<str>,
        _value: impl AsRef<[u8]>,
        _publish: bool,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        debug!(id, "process component delete");
        // TODO: TBD: stop actor if spec deleted?
        if let Some(_actor) = self.actors.write().await.get(id) {
            warn!("Component spec deleted but actor {} still running", id);
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
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

    #[instrument(level = "debug", skip_all)]
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
            }
            Claims::Provider(claims) => {
                let mut provider_claims = self.provider_claims.write().await;
                provider_claims.remove(&claims.subject);
            }
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    async fn process_entry(
        &self,
        KvEntry {
            key,
            value,
            operation,
            ..
        }: KvEntry,
        publish: bool,
    ) {
        let key_id = key.split_once('_');
        let res = match (operation, key_id) {
            (Operation::Put, Some(("COMPONENT", id))) => {
                self.process_component_spec_put(id, value, publish).await
            }
            (Operation::Delete, Some(("COMPONENT", id))) => {
                self.process_component_spec_delete(id, value, publish).await
            }
            (Operation::Put, Some(("LINKDEF", _id))) => {
                debug!("ignoring deprecated LINKDEF put operation");
                Ok(())
            }
            (Operation::Delete, Some(("LINKDEF", _id))) => {
                debug!("ignoring deprecated LINKDEF delete operation");
                Ok(())
            }
            (Operation::Put, Some(("CLAIMS", pubkey))) => {
                self.process_claims_put(pubkey, value).await
            }
            (Operation::Delete, Some(("CLAIMS", pubkey))) => {
                self.process_claims_delete(pubkey, value).await
            }
            (operation, Some(("REFMAP", id))) => {
                // TODO: process REFMAP entries
                debug!(?operation, id, "ignoring REFMAP entry");
                Ok(())
            }
            _ => {
                warn!(key, ?operation, "unsupported KV bucket entry");
                Ok(())
            }
        };
        if let Err(error) = &res {
            error!(key, ?operation, ?error, "failed to process KV bucket entry");
        }
    }

    #[instrument(level = "trace", skip_all, fields(bucket = %entry.bucket, key = %entry.key, revision = %entry.revision, operation = ?entry.operation))]
    async fn process_config_entry(&self, entry: KvEntry) {
        match entry.operation {
            Operation::Put => {
                let data: HashMap<String, String> = match serde_json::from_slice(&entry.value) {
                    Ok(data) => data,
                    Err(error) => {
                        error!(?error, "failed to decode config data on entry update");
                        return;
                    }
                };
                let mut lock = self.config_data_cache.write().await;
                lock.insert(entry.key, data);
            }
            Operation::Delete | Operation::Purge => {
                self.config_data_cache.write().await.remove(&entry.key);
            }
        }
    }
}

/// Helper function to serialize CtlResponse<T> into a Vec<u8> if the response is Some
fn serialize_ctl_response<T: Serialize>(
    ctl_response: Option<CtlResponse<T>>,
) -> Option<anyhow::Result<Vec<u8>>> {
    ctl_response.map(|resp| serde_json::to_vec(&resp).map_err(anyhow::Error::from))
}

// TODO: remove StoredClaims in #1093
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum StoredClaims {
    Actor(StoredActorClaims),
    Provider(StoredProviderClaims),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredActorClaims {
    call_alias: String,
    #[serde(alias = "caps", deserialize_with = "deserialize_messy_vec")]
    capabilities: Vec<String>,
    #[serde(alias = "iss")]
    issuer: String,
    name: String,
    #[serde(alias = "rev")]
    revision: String,
    #[serde(alias = "sub")]
    subject: String,
    #[serde(deserialize_with = "deserialize_messy_vec")]
    tags: Vec<String>,
    version: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredProviderClaims {
    contract_id: String,
    #[serde(alias = "iss")]
    issuer: String,
    name: String,
    #[serde(alias = "rev")]
    revision: String,
    #[serde(alias = "sub")]
    subject: String,
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
                Ok(StoredClaims::Actor(StoredActorClaims {
                    call_alias: call_alias.unwrap_or_default(),
                    capabilities: caps.unwrap_or_default(),
                    issuer,
                    name: name.unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject,
                    tags: tags.unwrap_or_default(),
                    version: ver.unwrap_or_default(),
                }))
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
                Ok(StoredClaims::Provider(StoredProviderClaims {
                    contract_id,
                    issuer,
                    name: name.unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject,
                    version: ver.unwrap_or_default(),
                    config_schema: config_schema.map(|schema| schema.to_string()),
                }))
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
                Ok(StoredClaims::Actor(StoredActorClaims {
                    call_alias: call_alias.clone().unwrap_or_default(),
                    capabilities: caps.clone().unwrap_or_default(),
                    issuer: issuer.clone(),
                    name: name.clone().unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject: subject.clone(),
                    tags: tags.clone().unwrap_or_default(),
                    version: ver.clone().unwrap_or_default(),
                }))
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
                Ok(StoredClaims::Provider(StoredProviderClaims {
                    contract_id: contract_id.clone(),
                    issuer: issuer.clone(),
                    name: name.clone().unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject: subject.clone(),
                    version: ver.clone().unwrap_or_default(),
                    config_schema: config_schema.as_ref().map(ToString::to_string),
                }))
            }
        }
    }
}

#[allow(clippy::implicit_hasher)]
impl From<StoredClaims> for HashMap<String, String> {
    fn from(claims: StoredClaims) -> Self {
        match claims {
            StoredClaims::Actor(claims) => HashMap::from([
                ("call_alias".to_string(), claims.call_alias),
                ("caps".to_string(), claims.capabilities.clone().join(",")), // TODO: remove in #1093
                ("capabilities".to_string(), claims.capabilities.join(",")),
                ("iss".to_string(), claims.issuer.clone()), // TODO: remove in #1093
                ("issuer".to_string(), claims.issuer),
                ("name".to_string(), claims.name),
                ("rev".to_string(), claims.revision.clone()), // TODO: remove in #1093
                ("revision".to_string(), claims.revision),
                ("sub".to_string(), claims.subject.clone()), // TODO: remove in #1093
                ("subject".to_string(), claims.subject),
                ("tags".to_string(), claims.tags.join(",")),
                ("version".to_string(), claims.version),
            ]),
            StoredClaims::Provider(claims) => HashMap::from([
                ("contract_id".to_string(), claims.contract_id),
                ("iss".to_string(), claims.issuer.clone()), // TODO: remove in #1093
                ("issuer".to_string(), claims.issuer),
                ("name".to_string(), claims.name),
                ("rev".to_string(), claims.revision.clone()), // TODO: remove in #1093
                ("revision".to_string(), claims.revision),
                ("sub".to_string(), claims.subject.clone()), // TODO: remove in #1093
                ("subject".to_string(), claims.subject),
                ("version".to_string(), claims.version),
                (
                    "config_schema".to_string(),
                    claims.config_schema.unwrap_or_default(),
                ),
            ]),
        }
    }
}

fn deserialize_messy_vec<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    MessyVec::deserialize(deserializer).map(|messy_vec| messy_vec.0)
}
// Helper struct to deserialize either a comma-delimited string or an actual array of strings
struct MessyVec(pub Vec<String>);

struct MessyVecVisitor;

// Since this is "temporary" code to preserve backwards compatibility with already-serialized claims,
// we use fully-qualified names instead of importing
impl<'de> serde::de::Visitor<'de> for MessyVecVisitor {
    type Value = MessyVec;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("string or array of strings")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut values = Vec::new();

        while let Some(value) = seq.next_element()? {
            values.push(value);
        }

        Ok(MessyVec(values))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(MessyVec(value.split(',').map(String::from).collect()))
    }
}

impl<'de> Deserialize<'de> for MessyVec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        deserializer.deserialize_any(MessyVecVisitor)
    }
}

fn human_friendly_uptime(uptime: Duration) -> String {
    // strip sub-seconds, then convert to human-friendly format
    humantime::format_duration(
        uptime.saturating_sub(Duration::from_nanos(uptime.subsec_nanos().into())),
    )
    .to_string()
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
