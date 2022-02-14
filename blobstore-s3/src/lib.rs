//! AWS S3 implementation for wasmcloud:blobstore
//!
//! TODO:
//! - multipart upload is not yet implemented, and has some complications:
//!   - the S3 api for multipart upload requires a minimum 5MB per "part",
//!     and the file may have up to 10,000 parts. With a default nats message limit of 1MB,
//!     chunks uploaded from an actor need to be aggregated either in memory,
//!     or as smaller unique S3 files that get copied into parts.
//!   - aggregating 5MB parts in the memory of the capability provider would be simple,
//!     but we can't keep state in provider memory because there could be multiple instances.
//!   - complete_multipart_upload can discard errors https://github.com/rusoto/rusoto/issues/1936
//!
//! assume role http request https://docs.aws.amazon.com/cli/latest/reference/sts/assume-role.html
//! get session token https://docs.aws.amazon.com/cli/latest/reference/sts/get-session-token.html

use aws_sdk_s3::{
    error::{HeadBucketError, HeadBucketErrorKind, HeadObjectError, HeadObjectErrorKind},
    model::ObjectIdentifier,
    output::{CreateBucketOutput, ListBucketsOutput},
    ByteStream, SdkError,
};
use log::{debug, error, info, warn};
use wasmbus_rpc::{core::LinkDefinition, provider::prelude::*};
use wasmcloud_interface_blobstore::{
    self as blobstore, Blobstore, Chunk, ChunkReceiver, ChunkReceiverSender, ContainerId,
    ContainerIds, ContainerMetadata, ContainerObject, ContainersInfo, GetObjectResponse,
    MultiResult, ObjectMetadata, PutChunkRequest, PutObjectResponse, RemoveObjectsRequest,
};
mod config;
pub use config::StorageConfig;

#[derive(Clone)]
pub struct StorageClient(pub aws_sdk_s3::Client, pub Option<LinkDefinition>);

impl StorageClient {
    pub async fn new(config: StorageConfig, ld: Option<LinkDefinition>) -> Self {
        let s3_config = aws_sdk_s3::Config::from(&config.configure_aws().await);
        let s3_client = aws_sdk_s3::Client::from_conf(s3_config);
        StorageClient(s3_client, ld)
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

    async fn stream_download(
        &self,
        ctx: &Context,
        container_object: ContainerObject,
        offset: usize,
        total_size: usize,
        mut stream: ByteStream,
    ) {
        let ctx = ctx.clone();
        let this = self.clone();
        let _join = tokio::spawn(async move {
            let actor_id = ctx.actor.as_ref().unwrap();
            let mut offset = offset;
            loop {
                if offset >= total_size {
                    break;
                }
                match tokio_stream::StreamExt::try_next(&mut stream).await {
                    Ok(Some(bytes)) => {
                        let chunk_len = bytes.len();
                        if chunk_len == 0 {
                            warn!("object stream returned zero bytes");
                            continue;
                        }
                        let chunk = Chunk {
                            is_last: (offset + chunk_len >= total_size),
                            bytes: bytes.to_vec(),
                            offset: offset as u64,
                            container_id: container_object.container_id.to_string(),
                            object_id: container_object.object_id.to_string(),
                        };
                        // increment for next iteration
                        offset += chunk_len;
                        let ld = this.1.clone().unwrap();
                        let receiver = ChunkReceiverSender::for_actor(&ld);
                        if let Err(e) = receiver.receive_chunk(&ctx, &chunk).await {
                            error!("sending chunk error. stopping streaming download. Bucket({}) Object({}) to Actor({}): {}",
                                    &container_object.container_id,
                                    &container_object.object_id,
                                    actor_id,
                                    e
                            );
                            break;
                        }
                    }
                    Ok(None) => {
                        // end of stream
                        break;
                    }
                    Err(e) => {
                        error!("object download stream failed: {}", e.to_string());
                        break;
                    }
                }
            }
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
        _arg: &ContainerId,
    ) -> RpcResult<ContainerMetadata> {
        unimplemented!()
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
                            size: o.size as u64,
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
                let mut results = Vec::with_capacity(arg.objects.len());
                if let Some(errors) = output.errors {
                    for e in errors.iter() {
                        results.push(blobstore::ItemResult {
                            key: e.key.clone().unwrap_or_default(),
                            error: e.message.clone(),
                            success: false,
                        });
                    }
                }
                if !results.is_empty() {
                    error!(
                        "delete_objects returned {}/{} errors",
                        results.len(),
                        arg.objects.len()
                    );
                }
                Ok(results)
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
        let get_object_req = self
            .0
            .get_object()
            .bucket(&arg.container_id)
            .key(&arg.object_id)
            .set_range(to_range_header(arg.range_start, arg.range_end));
        match get_object_req.send().await {
            Ok(mut object_output) => {
                let len = object_output.content_length as u64;
                let bytes = tokio_stream::StreamExt::try_next(&mut object_output.body)
                    .await
                    .map_err(|e| RpcError::Other(format!("io error reading object: {}", e)))?
                    .unwrap_or_default();
                let chunk_len = bytes.len();
                if (chunk_len as u64) < len {
                    info!("get_object Bucket({}) Object({}) beginning streaming response. Initial chunk contains {} bytes out of {}",
                        &arg.container_id,
                        &arg.object_id,
                        chunk_len, len
                    );
                    if self.1.is_some() {
                        // create task to deliver remaining chunks
                        self.stream_download(
                            ctx,
                            ContainerObject {
                                container_id: arg.container_id.clone(),
                                object_id: arg.object_id.clone(),
                            },
                            arg.range_start.unwrap_or(0) as usize + chunk_len,
                            len as usize,
                            object_output.body,
                        )
                        .await;
                    } else {
                        let msg = format!("Returning first chunk of {} bytes (out of {}) for Bucket({}) Object({}). Remaining chunks will not be sent to ChunkReceiver because linkdef was not initialized. This is most likely due to invoking 'getObject' with an improper configuration for testing.",
                            chunk_len, len,
                            &arg.container_id,
                            &arg.object_id,
                        );
                        error!("{}", &msg);
                    }
                }
                // return first chunk
                Ok(blobstore::GetObjectResponse {
                    success: true,
                    initial_chunk: Some(Chunk {
                        bytes: bytes.to_vec(),
                        container_id: arg.container_id.clone(),
                        is_last: (chunk_len as u64) >= len,
                        object_id: arg.object_id.clone(),
                        offset: arg.range_start.unwrap_or(0),
                    }),
                    content_length: object_output.content_length as u64,
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
fn to_timestamp(dt: Option<aws_sdk_s3::DateTime>) -> Option<wasmbus_rpc::Timestamp> {
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
    if bucket.starts_with("xn--") {
        return Err("bucket name must not begin with 'xn--'");
    }
    if bucket.ends_with("-s3alias") {
        return Err("bucket name must not end with '-s3alias'");
    }
    // there are a couple
    // IPv4 address is not allowed

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
