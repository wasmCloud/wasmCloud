//! AWS S3 implementation for wasmcloud:blobstore
//!
//! assume role http request https://docs.aws.amazon.com/cli/latest/reference/sts/assume-role.html
//! get session token https://docs.aws.amazon.com/cli/latest/reference/sts/get-session-token.html

use std::num::NonZeroU64;
use std::sync::Arc;
use std::{collections::HashMap, num::NonZeroUsize};

use aws_sdk_s3::{
    error::{HeadBucketError, HeadBucketErrorKind, HeadObjectError, HeadObjectErrorKind},
    model::ObjectIdentifier,
    output::{CreateBucketOutput, HeadObjectOutput, ListBucketsOutput},
    types::{ByteStream, SdkError},
};
use bytes::Bytes;
use tokio_stream::StreamExt;
use tracing::{debug, error, instrument, warn};
use tracing_futures::Instrument;
use wasmbus_rpc::{core::LinkDefinition, provider::prelude::*};

use crate::wasmcloud_interface_blobstore::{
    self as blobstore, Blobstore, Chunk, ChunkReceiver, ChunkReceiverSender, ContainerId,
    ContainerIds, ContainerMetadata, ContainerObject, ContainersInfo, GetObjectResponse,
    MultiResult, ObjectMetadata, PutChunkRequest, PutObjectResponse, RemoveObjectsRequest,
};

mod config;
pub use config::StorageConfig;

// this is not an external library - built locally via build.rs & codegen.toml
#[allow(dead_code)]
pub mod wasmcloud_interface_blobstore {
    include!(concat!(env!("OUT_DIR"), "/gen/blobstore.rs"));
}

const ALIAS_PREFIX: &str = "alias_";

/// number of items to return in get_objects if max_items not specified
const DEFAULT_MAX_ITEMS: i32 = 1000;

/// maximum size of message that we'll return from s3 (500MB)
const MAX_CHUNK_SIZE: usize = 500 * 1024 * 1024;

#[derive(Clone)]
pub struct StorageClient {
    s3_client: aws_sdk_s3::Client,
    ld: Arc<LinkDefinition>,
    aliases: Arc<HashMap<String, String>>,
}

impl StorageClient {
    pub async fn new(config: StorageConfig, ld: LinkDefinition) -> Self {
        let mut aliases = config.aliases.clone();
        let s3_config = aws_sdk_s3::Config::from(&config.configure_aws().await);
        let s3_client = aws_sdk_s3::Client::from_conf(s3_config);
        for (k, v) in ld.values.iter() {
            if let Some(alias) = k.strip_prefix(ALIAS_PREFIX) {
                if alias.is_empty() || v.is_empty() {
                    error!("invalid bucket alias_ key and value must not be empty");
                } else {
                    aliases.insert(alias.to_string(), v.to_string());
                }
            }
        }
        StorageClient {
            s3_client,
            ld: Arc::new(ld),
            aliases: Arc::new(aliases),
        }
    }

    /// perform alias lookup on bucket name
    /// This can be used either for giving shortcuts to actors in the linkdefs, for example:
    /// - actor could use bucket names "alias_today", "alias_images", etc. and the linkdef aliases
    ///   will remap them to the real bucket name
    /// The 'alias_' prefix is not required, so this also works as a general redirect capability
    pub(crate) fn unalias<'n, 's: 'n>(&'s self, bucket_or_alias: &'n str) -> &'n str {
        debug!(%bucket_or_alias, aliases = ?self.aliases);
        let name = bucket_or_alias
            .strip_prefix(ALIAS_PREFIX)
            .unwrap_or(bucket_or_alias);
        if let Some(name) = self.aliases.get(name) {
            name.as_ref()
        } else {
            name
        }
    }

    // allow overriding chunk size for testing
    fn max_chunk_size(&self) -> usize {
        if let Ok(var) = std::env::var("MAX_CHUNK_SIZE") {
            if let Ok(size) = var.parse::<u32>() {
                return size as usize;
            }
        }
        MAX_CHUNK_SIZE
    }

    /// Perform any cleanup necessary for a link + s3 connection
    pub async fn close(&self) {
        debug!(actor_id = %self.ld.actor_id, "blobstore-s3 dropping linkdef");
        // If there were any https clients, caches, or other link-specific data,
        // we would delete those here
    }

    /// Retrieves metadata about the object
    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor))]
    async fn get_object_metadata(
        &self,
        _ctx: &Context,
        bucket_id: &str,
        object_id: &str,
    ) -> Result<ObjectMetadata, RpcError> {
        let bucket_id = self.unalias(bucket_id);
        match self
            .s3_client
            .head_object()
            .bucket(bucket_id)
            .key(object_id)
            .send()
            .await
        {
            Ok(HeadObjectOutput {
                last_modified,
                content_length,
                content_type,
                content_encoding,
                ..
            }) => Ok(ObjectMetadata {
                container_id: bucket_id.to_string(),
                object_id: object_id.to_string(),
                last_modified: to_timestamp(last_modified),
                content_type,
                content_encoding,
                content_length: content_length as u64,
            }),
            Err(SdkError::ServiceError {
                err:
                    HeadObjectError {
                        kind: HeadObjectErrorKind::NotFound(_),
                        ..
                    },
                ..
            }) => Err(RpcError::Other(format!(
                "Not found: Bucket({}) Object({})",
                bucket_id, object_id,
            ))),
            Err(e) => Err(RpcError::Other(format!(
                "get_object_metadata for Bucket({}) Object({}): {}",
                bucket_id, object_id, e
            ))),
        }
    }

    /// Sends bytes to actor in a single rpc message.
    /// If successful, returns number of bytes sent (same as chunk.content_length)
    #[instrument(level = "debug", skip(self, ctx, chunk), fields(actor_id = ?ctx.actor, object_id = %chunk.object_id, container_id = %self.unalias(&chunk.container_id)))]
    async fn send_chunk(&self, ctx: &Context, mut chunk: Chunk) -> Result<u64, RpcError> {
        chunk.container_id = self.unalias(&chunk.container_id).to_string();
        let receiver = ChunkReceiverSender::for_actor(self.ld.as_ref());
        if let Err(e) = receiver.receive_chunk(ctx, &chunk).await {
            let err = format!(
                "sending chunk error: Bucket({}) Object({}) to Actor({}): {}",
                &chunk.container_id, &chunk.object_id, &self.ld.actor_id, e
            );
            error!(error = %e, "sending chunk error");
            Err(RpcError::Rpc(err))
        } else {
            Ok(chunk.bytes.len() as u64)
        }
    }

    /// send any-size array of bytes to actor via streaming api,
    /// as one or more chunks of length <= MAX_CHUNK_SIZE
    /// Returns total number of bytes sent: (bytes.len())
    #[instrument(level = "debug", skip(self, ctx, cobj, bytes), fields(actor_id = ?ctx.actor, bucket_id = %self.unalias(&cobj.container_id), object_id = %cobj.object_id))]
    async fn stream_bytes(
        &self,
        ctx: &Context,
        offset: u64,
        end_range: u64, // last byte (inclusive) in requested range
        cobj: &ContainerObject,
        bytes: &[u8],
    ) -> Result<u64, RpcError> {
        let bucket_id = self.unalias(&cobj.container_id);
        let mut bytes_sent = 0u64;
        let bytes_to_send = bytes.len() as u64;
        while bytes_sent < bytes_to_send {
            let chunk_offset = offset + bytes_sent;
            let chunk_len = (self.max_chunk_size() as u64).min(bytes_to_send - bytes_sent);
            let Some(chunk_bytes) = NonZeroU64::new(
                self.send_chunk(
                    ctx,
                    Chunk {
                        is_last: offset + chunk_len > end_range,
                        bytes: bytes[bytes_sent as usize..(bytes_sent + chunk_len) as usize]
                            .to_vec(),
                        offset: chunk_offset as u64,
                        container_id: bucket_id.to_string(),
                        object_id: cobj.object_id.clone(),
                    },
                )
                .await?,
            ) else {
                return Err(RpcError::InvalidParameter(
                    "sent chunk successfully but actor returned 0 bytes received.".to_string(),
                ));
            };
            bytes_sent += chunk_bytes.get();
        }
        Ok(bytes_sent)
    }

    /// Async tokio task to accept chunks from S3 and send to actor.
    /// `container_object` has the names of the container (bucket) and object to be streamed,
    /// `excess` contains optional bytes from the first s3 stream chunk that didn't fit
    ///    in the initial message
    /// `offset` is the current offset within object that we are returning to the actor
    ///    (on entry, this should be the initial range offset requested plus the number
    ///    of bytes already sent to the actor in the GetObjectResponse)
    /// `end_range` the byte offset (inclusive) of the last byte to be returned to the client
    async fn stream_from_s3(
        &self,
        ctx: &Context,
        mut container_object: ContainerObject,
        excess: Vec<u8>, // excess bytes from first chunk
        offset: u64,
        end_range: u64, // last object offset in requested range (inclusive),
        mut stream: ByteStream,
    ) {
        let ctx = ctx.clone();
        let this = self.clone();
        container_object.container_id = self.unalias(&container_object.container_id).to_string();
        let actor_id = ctx.actor.clone();
        let excess_len = excess.len();
        let _ = tokio::spawn(
            async move {
                let mut offset = offset;
                if !excess.is_empty() {
                    offset += this
                        .stream_bytes(&ctx, offset, end_range, &container_object, &excess)
                        .await?;
                    if offset > end_range {
                        return Ok::<(), RpcError>(());
                    }
                }

                while let Some(Ok(bytes)) = stream.next().await {
                    if bytes.is_empty() {
                        warn!("object stream returned zero bytes, quitting stream");
                        break;
                    }
                    offset += this
                        .stream_bytes(&ctx, offset, end_range, &container_object, &bytes)
                        .await?;
                    if offset > end_range {
                        break;
                    }
                }
                Ok(())
            }
            .instrument(tracing::debug_span!(
                "stream_from_s3",
                ?actor_id,
                ?excess_len,
                offset,
                end_range
            )),
        );
    }
}

#[async_trait]
impl Blobstore for StorageClient {
    /// Find out whether container exists
    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, bucket_id = %self.unalias(arg)))]
    async fn container_exists(&self, _ctx: &Context, arg: &ContainerId) -> RpcResult<bool> {
        let bucket_id = self.unalias(arg);
        match self.s3_client.head_bucket().bucket(bucket_id).send().await {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError {
                err:
                    HeadBucketError {
                        kind: HeadBucketErrorKind::NotFound(_),
                        ..
                    },
                ..
            }) => Ok(false),
            Err(e) => {
                error!(error = %e, "Unable to head bucket");
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    /// Creates container if it does not exist
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, bucket_id = %self.unalias(arg)))]
    async fn create_container(&self, ctx: &Context, arg: &ContainerId) -> RpcResult<()> {
        let bucket_id = self.unalias(arg);
        match self.container_exists(ctx, &bucket_id.to_string()).await {
            Ok(true) => Ok(()),
            _ => {
                if let Err(msg) = validate_bucket_name(bucket_id) {
                    error!("invalid bucket name");
                    return Err(RpcError::InvalidParameter(format!(
                        "Invalid bucket name Bucket({}): {}",
                        bucket_id, msg
                    )));
                }
                match self
                    .s3_client
                    .create_bucket()
                    .bucket(bucket_id)
                    .send()
                    .await
                {
                    Ok(CreateBucketOutput { location, .. }) => {
                        debug!(?location, "bucket created");
                        Ok(())
                    }
                    Err(SdkError::ServiceError { err, .. }) => {
                        error!(
                            error = %err,
                            "Got service error",
                        );
                        Err(RpcError::Other(err.to_string()))
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            "unexpected_error",
                        );
                        Err(RpcError::Other(e.to_string()))
                    }
                }
            }
        }
    }

    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, bucket_id = %self.unalias(arg)))]
    async fn get_container_info(
        &self,
        _ctx: &Context,
        arg: &ContainerId,
    ) -> RpcResult<ContainerMetadata> {
        let bucket_id = self.unalias(arg);
        match self.s3_client.head_bucket().bucket(bucket_id).send().await {
            Ok(_) => Ok(ContainerMetadata {
                container_id: bucket_id.to_string(),
                // unfortunately, HeadBucketOut doesn't include any information
                // so we can't fill in creation date
                created_at: None,
            }),
            Err(SdkError::ServiceError {
                err:
                    HeadBucketError {
                        kind: HeadBucketErrorKind::NotFound(_),
                        ..
                    },
                ..
            }) => Err(RpcError::Other(format!("Bucket({})not found", bucket_id))),
            Err(e) => Err(RpcError::Other(e.to_string())),
        }
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor))]
    async fn list_containers(&self, _ctx: &Context) -> RpcResult<ContainersInfo> {
        match self.s3_client.list_buckets().send().await {
            Ok(ListBucketsOutput {
                buckets: Some(list),
                ..
            }) => Ok(list
                .iter()
                .map(|bucket| ContainerMetadata {
                    container_id: bucket.name.clone().unwrap_or_default(),
                    created_at: to_timestamp(bucket.creation_date),
                })
                .collect()),
            Ok(ListBucketsOutput { buckets: None, .. }) => Ok(Vec::new()),
            Err(SdkError::ServiceError { err, .. }) => {
                error!(error = %err, "Service error");
                Err(RpcError::Other(err.to_string()))
            }
            Err(e) => {
                error!(error = %e, "unexpected error");
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor))]
    async fn remove_containers(
        &self,
        _ctx: &Context,
        arg: &ContainerIds,
    ) -> RpcResult<MultiResult> {
        let mut results = Vec::with_capacity(arg.len());
        for bucket in arg.iter() {
            let bucket = self.unalias(bucket);
            match self.s3_client.delete_bucket().bucket(bucket).send().await {
                Ok(_) => {}
                Err(SdkError::ServiceError { err, .. }) => {
                    results.push(blobstore::ItemResult {
                        key: bucket.to_string(),
                        error: Some(err.to_string()),
                        success: false,
                    });
                }
                Err(e) => {
                    error!(error = %e, "unexpected error");
                    return Err(RpcError::Other(format!("unexpected error: {}", e)));
                }
            }
        }
        if !results.is_empty() {
            error!(
                "remove_containers returned {}/{} errors",
                results.len(),
                arg.len()
            );
        }
        Ok(results)
    }

    /// Find out whether object exists
    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, bucket_id = %self.unalias(&arg.container_id), object_id = %arg.object_id))]
    async fn object_exists(&self, _ctx: &Context, arg: &ContainerObject) -> RpcResult<bool> {
        let bucket_id = self.unalias(&arg.container_id);
        match self
            .s3_client
            .head_object()
            .bucket(bucket_id)
            .key(&arg.object_id)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError {
                err:
                    HeadObjectError {
                        kind: HeadObjectErrorKind::NotFound(_),
                        ..
                    },
                ..
            }) => Ok(false),
            Err(e) => {
                error!(
                    error = %e,
                    "unexpected error for object_exists"
                );
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    /// Retrieves metadata about the object
    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, bucket_id = %self.unalias(&arg.container_id), object_id = %arg.object_id))]
    async fn get_object_info(
        &self,
        _ctx: &Context,
        arg: &ContainerObject,
    ) -> Result<ObjectMetadata, RpcError> {
        let bucket_id = self.unalias(&arg.container_id);
        match self
            .s3_client
            .head_object()
            .bucket(bucket_id)
            .key(arg.object_id.clone())
            .send()
            .await
        {
            Ok(HeadObjectOutput {
                last_modified,
                content_length,
                content_type,
                content_encoding,
                ..
            }) => Ok(ObjectMetadata {
                container_id: bucket_id.to_string(),
                object_id: arg.object_id.clone(),
                last_modified: to_timestamp(last_modified),
                content_type,
                content_encoding,
                content_length: content_length as u64,
            }),
            Err(SdkError::ServiceError {
                err:
                    HeadObjectError {
                        kind: HeadObjectErrorKind::NotFound(_),
                        ..
                    },
                ..
            }) => Err(RpcError::Other(format!(
                "Not found: Bucket({}) Object({})",
                bucket_id, &arg.object_id,
            ))),
            Err(e) => Err(RpcError::Other(format!(
                "get_object_metadata for Bucket({}) Object({}): {}",
                bucket_id, &arg.object_id, e
            ))),
        }
    }

    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, bucket_id = %self.unalias(&arg.container_id), max_items = arg.max_items))]
    async fn list_objects(
        &self,
        _ctx: &Context,
        arg: &blobstore::ListObjectsRequest,
    ) -> RpcResult<blobstore::ListObjectsResponse> {
        let bucket_id = self.unalias(&arg.container_id);
        debug!("asking for list_objects bucket: {}", bucket_id);
        let mut req = self.s3_client.list_objects_v2().bucket(bucket_id);
        if let Some(max_items) = arg.max_items {
            if max_items > i32::MAX as u32 {
                // edge case to avoid panic
                return Err(RpcError::InvalidParameter(
                    "max_items too large".to_string(),
                ));
            }
            req = req.max_keys(max_items as i32);
        } else {
            req = req.max_keys(DEFAULT_MAX_ITEMS);
        }
        if let Some(continuation) = &arg.continuation {
            req = req.set_continuation_token(Some(continuation.clone()));
        } else if let Some(start_with) = &arg.start_with {
            req = req.set_start_after(Some(start_with.clone()));
        }
        match req.send().await {
            Ok(list) => {
                debug!(
                    "list_objects (bucket:{}) returned {} items",
                    bucket_id,
                    list.contents.as_ref().map(|l| l.len()).unwrap_or(0)
                );
                let is_last = !list.is_truncated;
                let objects = match list.contents {
                    Some(items) => items
                        .iter()
                        .map(|o| ObjectMetadata {
                            container_id: bucket_id.to_string(),
                            last_modified: to_timestamp(o.last_modified),
                            object_id: o.key.clone().unwrap_or_default(),
                            content_length: o.size as u64,
                            content_encoding: None,
                            content_type: None,
                        })
                        .collect(),
                    None => Vec::<ObjectMetadata>::new(),
                };
                Ok(blobstore::ListObjectsResponse {
                    continuation: list.next_continuation_token,
                    objects,
                    is_last,
                })
            }
            Err(e) => {
                error!(error = %e, "unable to list objects");
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, bucket_id = %self.unalias(&arg.container_id)))]
    async fn remove_objects(
        &self,
        _ctx: &Context,
        arg: &RemoveObjectsRequest,
    ) -> RpcResult<MultiResult> {
        let bucket_id = self.unalias(&arg.container_id);
        match self
            .s3_client
            .delete_objects()
            .bucket(bucket_id)
            .delete(
                aws_sdk_s3::model::Delete::builder()
                    .set_objects(Some(
                        arg.objects
                            .iter()
                            .map(|id| ObjectIdentifier::builder().key(id).build())
                            .collect(),
                    ))
                    .quiet(true)
                    .build(),
            )
            .send()
            .await
        {
            Ok(output) => {
                if let Some(errors) = output.errors {
                    let mut results = Vec::with_capacity(errors.len());
                    for e in errors.iter() {
                        results.push(blobstore::ItemResult {
                            key: e.key.clone().unwrap_or_default(),
                            error: e.message.clone(),
                            success: false,
                        });
                    }
                    if !results.is_empty() {
                        error!(
                            "delete_objects returned {}/{} errors",
                            results.len(),
                            arg.objects.len()
                        );
                    }
                    Ok(results)
                } else {
                    Ok(Vec::new())
                }
            }
            Err(e) => {
                error!(error = %e, "Unable to delete objects");
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    #[instrument(
        level = "debug",
        skip(self, _ctx, arg),
        fields(actor_id = ?_ctx.actor, bucket_id = %self.unalias(&arg.chunk.container_id), object_id = %arg.chunk.object_id, offset = %arg.chunk.offset, is_last = %arg.chunk.is_last)
    )]
    async fn put_object(
        &self,
        _ctx: &Context,
        arg: &blobstore::PutObjectRequest,
    ) -> RpcResult<PutObjectResponse> {
        let bucket_id = self.unalias(&arg.chunk.container_id);
        if !arg.chunk.is_last {
            error!("put_object for multi-part upload: not implemented!");
            return Err(RpcError::InvalidParameter(
                "multipart upload not implemented".to_string(),
            ));
        }
        if arg.chunk.offset != 0 {
            error!("put_object with initial offset non-zero: not implemented!");
            return Err(RpcError::InvalidParameter(
                "non-zero offset not supported".to_string(),
            ));
        }
        if arg.chunk.bytes.is_empty() {
            error!("put_object with zero bytes");
            return Err(RpcError::InvalidParameter(
                "cannot put zero-length objects".to_string(),
            ));
        }
        // TODO: make sure put_object takes an owned `PutObjectRequest` to avoid cloning the whole chunk
        let bytes = arg.chunk.bytes.to_owned();
        match self
            .s3_client
            .put_object()
            .bucket(bucket_id)
            .key(&arg.chunk.object_id)
            .body(ByteStream::from(bytes))
            .send()
            .await
        {
            Ok(_) => Ok(PutObjectResponse::default()),
            Err(e) => {
                error!(
                    error = %e,
                    "Error putting object",
                );
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    /// Retrieve object from s3 storage.
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, bucket_id = %self.unalias(&arg.container_id), object_id = %arg.object_id, bytes_requested = tracing::field::Empty))]
    async fn get_object(
        &self,
        ctx: &Context,
        arg: &blobstore::GetObjectRequest,
    ) -> RpcResult<GetObjectResponse> {
        let bucket_id = self.unalias(&arg.container_id);
        let max_chunk_size = self.max_chunk_size();
        // If the object is not found, or not readable, get_object_metadata will return error.
        let meta = self
            .get_object_metadata(ctx, bucket_id, &arg.object_id)
            .await?;
        // calculate content_length requested, with error checking for range bounds
        let bytes_requested = match (arg.range_start, arg.range_end) {
            (None, Some(end)) => meta.content_length.min(end + 1),
            (Some(start), None) if start < meta.content_length => meta.content_length - start,
            (Some(start), Some(end)) if (start <= end) && start < meta.content_length => {
                meta.content_length.min(end - start + 1)
            }
            (None, None) => meta.content_length,
            _ => 0,
        };
        tracing::span::Span::current().record(
            "bytes_requested",
            &tracing::field::display(&bytes_requested),
        );
        if bytes_requested == 0 {
            return Ok(GetObjectResponse {
                content_length: 0,
                content_encoding: meta.content_encoding.clone(),
                content_type: meta.content_type.clone(),
                initial_chunk: Some(Chunk {
                    bytes: vec![],
                    container_id: bucket_id.to_string(),
                    object_id: arg.object_id.clone(),
                    is_last: true,
                    offset: 0,
                }),
                success: true,
                error: None,
            });
        }

        let get_object_req = self
            .s3_client
            .get_object()
            .bucket(bucket_id)
            .key(&arg.object_id)
            .set_range(to_range_header(arg.range_start, arg.range_end));
        match get_object_req.send().await {
            Ok(mut object_output) => {
                let len = object_output.content_length as u64;
                if len > bytes_requested {
                    // either the math is wrong above, or we misunderstood the api.
                    // docs say content_length is "Size of the body in bytes"
                    error!(
                        %len,
                        "requested {} bytes but more bytes were returned!",
                        bytes_requested
                    );
                }
                let mut bytes = match object_output.body.next().await {
                    Some(Ok(bytes)) => {
                        debug!(chunk_len = %bytes.len(), "initial chunk received");
                        bytes
                    }
                    None => {
                        error!("stream ended before getting first chunk from s3");
                        return Err(RpcError::Other("no data received from s3".to_string()));
                    }
                    Some(Err(e)) => {
                        error!(error = %e, "chunk.try_next returned error");
                        return Err(RpcError::Other(e.to_string()));
                    }
                };
                // determine if we need to stream additional chunks
                let bytes = if (bytes.len() as u64) < bytes_requested
                    || bytes.len() > max_chunk_size as usize
                {
                    debug!(
                        chunk_len = %bytes.len(),
                        "Beginning streaming response. Initial S3 chunk contains {} bytes out of {}",
                        bytes.len(),
                        bytes_requested,
                    );
                    let (bytes, excess) = if bytes.len() > max_chunk_size {
                        let excess = bytes.split_off(max_chunk_size);
                        (bytes, excess)
                    } else {
                        (bytes, Bytes::new())
                    };
                    // create task to deliver remaining chunks
                    let offset = arg.range_start.unwrap_or(0) + bytes.len() as u64;
                    self.stream_from_s3(
                        ctx,
                        ContainerObject {
                            container_id: bucket_id.to_string(),
                            object_id: arg.object_id.clone(),
                        },
                        excess.into(),
                        offset,
                        offset + bytes_requested,
                        object_output.body,
                    )
                    .await;
                    Vec::from(bytes)
                } else {
                    // no streaming required - everything in first chunk
                    Vec::from(bytes)
                };
                // return first chunk
                Ok(blobstore::GetObjectResponse {
                    success: true,
                    initial_chunk: Some(Chunk {
                        is_last: (bytes.len() as u64) >= bytes_requested,
                        bytes,
                        container_id: bucket_id.to_string(),
                        object_id: arg.object_id.clone(),
                        offset: arg.range_start.unwrap_or(0),
                    }),
                    content_length: bytes_requested,
                    content_type: object_output.content_type.clone(),
                    content_encoding: object_output.content_encoding.clone(),
                    error: None,
                })
            }
            Err(e) => {
                error!(
                    error = %e,
                    "Error when getting object"
                );
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    async fn put_chunk(&self, _ctx: &Context, _arg: &PutChunkRequest) -> RpcResult<()> {
        error!("put_chunk is unimplemented");
        Err(RpcError::NotImplemented)
    }
}

/// translate optional s3 DateTime to optional Timestamp.
/// Invalid times return None.
fn to_timestamp(dt: Option<aws_sdk_s3::types::DateTime>) -> Option<wasmbus_rpc::Timestamp> {
    match dt {
        Some(dt) => match wasmbus_rpc::Timestamp::new(dt.secs(), dt.subsec_nanos()) {
            Ok(t) => Some(t),
            Err(_) => None,
        },
        None => None,
    }
}

// enforce some of the S3 bucket naming rules.
// per https://docs.aws.amazon.com/AmazonS3/latest/userguide/bucketnamingrules.html
// We don't enforce all of them (assuming amazon will also return an error),
// and we only enforce during `create_bucket`
fn validate_bucket_name(bucket: &str) -> Result<(), &'static str> {
    if !(3usize..=63).contains(&bucket.len()) {
        return Err("bucket name must be between 3(min) and 63(max) characters");
    }
    if !bucket
        .chars()
        .all(|c| c == '.' || c == '-' || ('a'..='z').contains(&c) || ('0'..='9').contains(&c))
    {
        return Err(
            "bucket names can only contain lowercase letters, numbers, dots('.') and hyphens('-')",
        );
    }
    let c = bucket.chars().next().unwrap();
    if !(('a'..='z').contains(&c) || ('0'..='9').contains(&c)) {
        return Err("bucket names must begin with a letter or number");
    }
    let c = bucket.chars().last().unwrap();
    if !(('a'..='z').contains(&c) || ('0'..='9').contains(&c)) {
        return Err("bucket names must end with a letter or number");
    }

    // Not all s3 validity rules are enforced here. For example, a plain IPv4 address is not allowed.
    // Rather than keep up with all the rules, we'll let the aws sdk check and return other errors.

    Ok(())
}

/// convert optional start/end to an http range request header value
/// If end is before start, the range is invalid, and per spec (https://www.w3.org/Protocols/rfc2616/rfc2616-sec14.html#sec14.35),
/// the range will be ignored.
/// If end is specified and start is None, start value of 0 is used. (Otherwise "bytes=-x" is interpreted as the last x bytes)
fn to_range_header(start: Option<u64>, end: Option<u64>) -> Option<String> {
    match (start, end) {
        (Some(start), Some(end)) if start <= end => Some(format!("bytes={}-{}", start, end)),
        (Some(start), None) => Some(format!("bytes={}-", start)),
        (None, Some(end)) => Some(format!("bytes=0-{}", end)),
        _ => None,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn range_header() {
        assert_eq!(
            to_range_header(Some(1), Some(99)),
            Some("bytes=1-99".to_string())
        );
        assert_eq!(to_range_header(Some(10), Some(5)), None);
        assert_eq!(
            to_range_header(None, Some(99)),
            Some("bytes=0-99".to_string())
        );
        assert_eq!(
            to_range_header(Some(99), None),
            Some("bytes=99-".to_string())
        );
        assert_eq!(to_range_header(None, None), None);
    }

    #[test]
    fn bucket_name() {
        assert!(validate_bucket_name("ok").is_err(), "too short");
        assert!(
            validate_bucket_name(&format!("{:65}", 'a')).is_err(),
            "too long"
        );
        assert!(validate_bucket_name("abc").is_ok());
        assert!(
            validate_bucket_name("abc.def-ghijklmnopqrstuvwxyz.1234567890").is_ok(),
            "valid chars"
        );
        assert!(validate_bucket_name("hasCAPS").is_err(), "no caps");
        assert!(
            validate_bucket_name("has_underscpre").is_err(),
            "no underscore"
        );
        assert!(
            validate_bucket_name(".not.ok").is_err(),
            "no start with dot"
        );
        assert!(validate_bucket_name("not.ok.").is_err(), "no end with dot");
    }

    #[tokio::test]
    async fn aliases() {
        let mut map = HashMap::new();
        map.insert(format!("{}foo", ALIAS_PREFIX), "bar".to_string());
        let mut ld = LinkDefinition::default();
        ld.values = map;
        let client = StorageClient::new(StorageConfig::default(), ld).await;

        // no alias
        assert_eq!(client.unalias("boo"), "boo");
        // alias without prefix
        assert_eq!(client.unalias("foo"), "bar");
        // alias with prefix
        assert_eq!(client.unalias(&format!("{}foo", ALIAS_PREFIX)), "bar");
        // undefined alias
        assert_eq!(client.unalias(&format!("{}baz", ALIAS_PREFIX)), "baz");
    }
}
