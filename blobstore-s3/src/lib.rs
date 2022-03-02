//! AWS S3 implementation for wasmcloud:blobstore
//!
//! assume role http request https://docs.aws.amazon.com/cli/latest/reference/sts/assume-role.html
//! get session token https://docs.aws.amazon.com/cli/latest/reference/sts/get-session-token.html

use crate::wasmcloud_interface_blobstore::{
    self as blobstore, Blobstore, Chunk, ChunkReceiver, ChunkReceiverSender, ContainerId,
    ContainerIds, ContainerMetadata, ContainerObject, ContainersInfo, GetObjectResponse,
    MultiResult, ObjectMetadata, PutChunkRequest, PutObjectResponse, RemoveObjectsRequest,
};
use aws_sdk_s3::{
    error::{HeadBucketError, HeadBucketErrorKind, HeadObjectError, HeadObjectErrorKind},
    model::ObjectIdentifier,
    output::{CreateBucketOutput, HeadObjectOutput, ListBucketsOutput},
    types::{ByteStream, SdkError},
};
use log::{debug, error, info, warn};
use tokio_stream::StreamExt;
use wasmbus_rpc::{core::LinkDefinition, provider::prelude::*};

mod config;
pub use config::StorageConfig;

// this is not an external library - built locally via build.rs & codegen.toml
#[allow(dead_code)]
pub mod wasmcloud_interface_blobstore {
    include!(concat!(env!("OUT_DIR"), "/gen/blobstore.rs"));
}

/// maximum size of message that we'll return from s3 (500MB)
const MAX_CHUNK_SIZE: usize = 500 * 1024 * 1024;

#[derive(Clone)]
pub struct StorageClient(pub aws_sdk_s3::Client, pub Option<LinkDefinition>);

impl StorageClient {
    pub async fn new(config: StorageConfig, ld: Option<LinkDefinition>) -> Self {
        let s3_config = aws_sdk_s3::Config::from(&config.configure_aws().await);
        let s3_client = aws_sdk_s3::Client::from_conf(s3_config);
        StorageClient(s3_client, ld)
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

    /// async implementation of Default
    pub async fn async_default() -> Self {
        Self::new(StorageConfig::default(), None).await
    }

    /// Perform any cleanup necessary for a link + s3 connection
    pub async fn close(&self) {
        if let Some(ld) = &self.1 {
            debug!("blobstore-s3 dropping linkdef for {}", ld.actor_id);
        }
        // If there were any https clients, caches, or other link-specific data,
        // we would delete those here
    }

    /// Retrieves metadata about the object
    async fn get_object_metadata(
        &self,
        _ctx: &Context,
        bucket_id: &str,
        object_id: &str,
    ) -> Result<ObjectMetadata, RpcError> {
        match self
            .0
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
    async fn send_chunk(&self, ctx: &Context, chunk: Chunk) -> Result<u64, RpcError> {
        let ld = self.1.clone().unwrap();
        let receiver = ChunkReceiverSender::for_actor(&ld);
        if let Err(e) = receiver.receive_chunk(ctx, &chunk).await {
            let err = format!(
                "sending chunk error: Bucket({}) Object({}) to Actor({}): {}",
                &chunk.container_id, &chunk.object_id, &ld.actor_id, e
            );
            error!("{}", &err);
            Err(RpcError::Rpc(err))
        } else {
            Ok(chunk.bytes.len() as u64)
        }
    }

    /// send any-size array of bytes to actor via streaming api,
    /// as one or more chunks of length <= MAX_CHUNK_SIZE
    /// Returns total number of bytes sent: (bytes.len())
    async fn stream_bytes(
        &self,
        ctx: &Context,
        offset: u64,
        end_range: u64, // last byte (inclusive) in requested range
        cobj: &ContainerObject,
        bytes: &[u8],
    ) -> Result<u64, RpcError> {
        let mut bytes_sent = 0u64;
        let bytes_to_send = bytes.len() as u64;
        while bytes_sent < bytes_to_send {
            let chunk_offset = offset + bytes_sent;
            let chunk_len = (self.max_chunk_size() as u64).min(bytes_to_send - bytes_sent);
            bytes_sent += self
                .send_chunk(
                    ctx,
                    Chunk {
                        is_last: offset + chunk_len > end_range,
                        bytes: bytes[bytes_sent as usize..(bytes_sent + chunk_len) as usize]
                            .to_vec(),
                        offset: chunk_offset as u64,
                        container_id: cobj.container_id.clone(),
                        object_id: cobj.object_id.clone(),
                    },
                )
                .await?;
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
        container_object: ContainerObject,
        excess: Vec<u8>, // excess bytes from first chunk
        offset: u64,
        end_range: u64, // last object offset in requested range (inclusive),
        mut stream: ByteStream,
    ) {
        let ctx = ctx.clone();
        let this = self.clone();
        let _ = tokio::spawn(async move {
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
        });
    }
}

#[async_trait]
impl Blobstore for StorageClient {
    /// Find out whether container exists
    async fn container_exists(&self, _ctx: &Context, arg: &ContainerId) -> RpcResult<bool> {
        match self.0.head_bucket().bucket(arg).send().await {
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
                error!("container_exists Bucket({}): error: {}", arg, e);
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    /// Creates container if it does not exist
    async fn create_container(&self, ctx: &Context, arg: &ContainerId) -> RpcResult<()> {
        match self.container_exists(ctx, arg).await {
            Ok(true) => Ok(()),
            _ => {
                if let Err(msg) = validate_bucket_name(arg) {
                    error!("invalid bucket name: {}", arg);
                    return Err(RpcError::InvalidParameter(format!(
                        "Invalid bucket name Bucket({}): {}",
                        arg, msg
                    )));
                }
                match self.0.create_bucket().bucket(arg).send().await {
                    Ok(CreateBucketOutput { location, .. }) => {
                        debug!("bucket created in {}", location.unwrap_or_default());
                        Ok(())
                    }
                    Err(SdkError::ServiceError { err, .. }) => {
                        error!("create_container Bucket({}): {}", arg, &err.to_string());
                        Err(RpcError::Other(err.to_string()))
                    }
                    Err(e) => {
                        error!(
                            "create_container Bucket({}) unexpected_error: {}",
                            arg,
                            &e.to_string()
                        );
                        Err(RpcError::Other(e.to_string()))
                    }
                }
            }
        }
    }

    async fn get_container_info(
        &self,
        _ctx: &Context,
        arg: &ContainerId,
    ) -> RpcResult<ContainerMetadata> {
        match self.0.head_bucket().bucket(arg).send().await {
            Ok(_) => Ok(ContainerMetadata {
                container_id: arg.to_string(),
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
            }) => Err(RpcError::Other(format!("Bucket({})not found", arg))),
            Err(e) => Err(RpcError::Other(e.to_string())),
        }
    }

    async fn list_containers(&self, _ctx: &Context) -> RpcResult<ContainersInfo> {
        match self.0.list_buckets().send().await {
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
                error!("list_containers: {}", &err.to_string());
                Err(RpcError::Other(err.to_string()))
            }
            Err(e) => {
                error!("list_containers: unexpected error {}", &e.to_string());
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    async fn remove_containers(
        &self,
        _ctx: &Context,
        arg: &ContainerIds,
    ) -> RpcResult<MultiResult> {
        let mut results = Vec::with_capacity(arg.len());
        for bucket in arg.iter() {
            match self.0.delete_bucket().bucket(bucket).send().await {
                Ok(_) => {}
                Err(SdkError::ServiceError { err, .. }) => {
                    results.push(blobstore::ItemResult {
                        key: bucket.clone(),
                        error: Some(err.to_string()),
                        success: false,
                    });
                }
                Err(e) => {
                    error!("remove_containers: unexpected error: {}", &e.to_string());
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
    async fn object_exists(&self, _ctx: &Context, arg: &ContainerObject) -> RpcResult<bool> {
        match self
            .0
            .head_object()
            .bucket(&arg.container_id)
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
                    "unexpected error for object_exists Bucket({}) Object({}): error: {}",
                    arg.container_id, arg.object_id, e
                );
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    /// Retrieves metadata about the object
    async fn get_object_info(
        &self,
        _ctx: &Context,
        arg: &ContainerObject,
    ) -> Result<ObjectMetadata, RpcError> {
        match self
            .0
            .head_object()
            .bucket(arg.container_id.clone())
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
                container_id: arg.container_id.clone(),
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
                &arg.container_id, &arg.object_id,
            ))),
            Err(e) => Err(RpcError::Other(format!(
                "get_object_metadata for Bucket({}) Object({}): {}",
                &arg.container_id, &arg.object_id, e
            ))),
        }
    }

    async fn list_objects(
        &self,
        _ctx: &Context,
        arg: &blobstore::ListObjectsRequest,
    ) -> RpcResult<blobstore::ListObjectsResponse> {
        let mut req = self.0.list_objects_v2().bucket(&arg.container_id);
        if let Some(max_items) = arg.max_items {
            if max_items > i32::MAX as u32 {
                // edge case to avoid panic
                return Err(RpcError::InvalidParameter(
                    "max_items too large".to_string(),
                ));
            }
            req = req.max_keys(max_items as i32);
        }
        if let Some(continuation) = &arg.continuation {
            req = req.set_continuation_token(Some(continuation.clone()));
        } else if let Some(start_with) = &arg.start_with {
            req = req.set_start_after(Some(start_with.clone()));
        }
        match req.send().await {
            Ok(list) => {
                let is_last = !list.is_truncated;
                let objects = match list.contents {
                    Some(items) => items
                        .iter()
                        .map(|o| ObjectMetadata {
                            container_id: arg.container_id.clone(),
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
                error!(
                    "list_objects Bucket({}): {}",
                    &arg.container_id,
                    &e.to_string(),
                );
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    async fn remove_objects(
        &self,
        _ctx: &Context,
        arg: &RemoveObjectsRequest,
    ) -> RpcResult<MultiResult> {
        match self
            .0
            .delete_objects()
            .bucket(&arg.container_id)
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
                error!("delete_objects: {}", &e.to_string());
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    async fn put_object(
        &self,
        _ctx: &Context,
        arg: &blobstore::PutObjectRequest,
    ) -> RpcResult<PutObjectResponse> {
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
        let bytes = arg.chunk.bytes.to_owned();
        match self
            .0
            .put_object()
            .bucket(&arg.chunk.container_id)
            .key(&arg.chunk.object_id)
            .body(ByteStream::from(bytes))
            .send()
            .await
        {
            Ok(_) => Ok(PutObjectResponse::default()),
            Err(e) => {
                error!(
                    "put_object: Bucket({}) Object({}): {}",
                    &arg.chunk.container_id,
                    &arg.chunk.object_id,
                    &e.to_string(),
                );
                Err(RpcError::Other(e.to_string()))
            }
        }
    }

    /// Retrieve object from s3 storage.
    async fn get_object(
        &self,
        ctx: &Context,
        arg: &blobstore::GetObjectRequest,
    ) -> RpcResult<GetObjectResponse> {
        let max_chunk_size = self.max_chunk_size();
        // If the object is not found, or not readable, get_object_metadata will return error.
        let meta = self
            .get_object_metadata(ctx, &arg.container_id, &arg.object_id)
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
        if bytes_requested == 0 {
            return Ok(GetObjectResponse {
                content_length: 0,
                content_encoding: meta.content_encoding.clone(),
                content_type: meta.content_type.clone(),
                initial_chunk: Some(Chunk {
                    bytes: vec![],
                    container_id: arg.container_id.clone(),
                    object_id: arg.object_id.clone(),
                    is_last: true,
                    offset: 0,
                }),
                success: true,
                error: None,
            });
        }
        let get_object_req = self
            .0
            .get_object()
            .bucket(&arg.container_id)
            .key(&arg.object_id)
            .set_range(to_range_header(arg.range_start, arg.range_end));
        match get_object_req.send().await {
            Ok(mut object_output) => {
                let len = object_output.content_length as u64;
                if len > bytes_requested {
                    // either the math is wrong above, or we misunderstood the api.
                    // docs say content_length is "Size of the body in bytes"
                    error!(
                        "requested {} bytes but more bytes {} were returned!",
                        bytes_requested, len
                    );
                }
                let bytes: Vec<u8> = match object_output.body.next().await {
                    Some(Ok(bytes)) => {
                        debug!("initial chunk len {} received", bytes.len());
                        bytes.to_vec()
                    }
                    None => {
                        error!("stream ended before getting first chunk from s3");
                        return Err(RpcError::Other("no data received from s3".to_string()));
                    }
                    Some(Err(e)) => {
                        error!("chunk.try_next returned {}", e.to_string());
                        return Err(RpcError::Other(e.to_string()));
                    }
                };
                // determine if we need to stream additional chunks
                let bytes = if (bytes.len() as u64) < bytes_requested
                    || bytes.len() > max_chunk_size as usize
                {
                    info!(
                        "get_object Bucket({}) Object({}) beginning streaming response. Initial \
                         S3 chunk contains {} bytes out of {}",
                        &arg.container_id,
                        &arg.object_id,
                        bytes.len(),
                        bytes_requested,
                    );
                    let (bytes, excess) = if bytes.len() > max_chunk_size {
                        (
                            bytes[..max_chunk_size].to_vec(),
                            bytes[max_chunk_size..].to_vec(),
                        )
                    } else {
                        (bytes, Vec::new())
                    };
                    if self.1.is_some() {
                        // create task to deliver remaining chunks
                        let offset = arg.range_start.unwrap_or(0) + bytes.len() as u64;
                        self.stream_from_s3(
                            ctx,
                            ContainerObject {
                                container_id: arg.container_id.clone(),
                                object_id: arg.object_id.clone(),
                            },
                            excess.to_vec(),
                            offset,
                            offset + bytes_requested,
                            object_output.body,
                        )
                        .await;
                    } else {
                        let msg = format!(
                            r#"Returning first chunk of {} bytes (out of {}) for 
                               Bucket({}) Object({}). 
                               Remaining chunks will not be sent to ChunkReceiver
                               because linkdef was not initialized. This is most likely due to
                               invoking 'getObject' with an improper configuration for testing."#,
                            bytes.len(),
                            bytes_requested,
                            &arg.container_id,
                            &arg.object_id,
                        );
                        error!("{}", &msg);
                    }
                    bytes
                } else {
                    // no streaming required - everything in first chunk
                    bytes
                };
                // return first chunk
                Ok(blobstore::GetObjectResponse {
                    success: true,
                    initial_chunk: Some(Chunk {
                        is_last: (bytes.len() as u64) >= bytes_requested,
                        bytes,
                        container_id: arg.container_id.clone(),
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
                    "get_object Bucket({}) Object({}): {}",
                    &arg.container_id,
                    &arg.object_id,
                    &e.to_string()
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
