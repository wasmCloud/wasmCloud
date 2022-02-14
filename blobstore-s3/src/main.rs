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

// using IAM role in AWS CLI https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-role.html
// assume IAM role using AWS CLI https://aws.amazon.com/premiumsupport/knowledge-center/iam-assume-role-cli/
// https://docs.aws.amazon.com/cli/latest/reference/sts/assume-role.html

// Principal for STSAssumeRole
// Principal/role : arn:aws:iam::123456789012:role/some-role
// Principal/service: "ec2.amazonaws.com"
// activate STS for region
// region: 'us-east-1'
// endpoint: 'https://sts.us-east-1.amazonaws.com'

use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::sync::RwLock;
use wasmbus_rpc::{core::LinkDefinition, provider::prelude::*};
use wasmcloud_interface_blobstore::{
    Blobstore, BlobstoreReceiver, ContainerId, ContainerIds, ContainerMetadata, ContainerObject,
    ContainersInfo, GetObjectRequest, GetObjectResponse, ListObjectsRequest, ListObjectsResponse,
    MultiResult, PutChunkRequest, PutObjectRequest, PutObjectResponse, RemoveObjectsRequest,
};
//mod config;
//use config::StorageConfig;
use blobstore_s3_lib::{StorageClient, StorageConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // handle lattice control messages and forward rpc to the provider dispatch
    // returns when provider receives a shutdown control message
    provider_main(S3BlobstoreProvider::default())?;

    eprintln!("blobstore-s3 provider exiting");
    Ok(())
}

/// Nats implementation for wasmcloud:messaging
#[derive(Default, Clone, Provider)]
#[services(Blobstore)]
struct S3BlobstoreProvider {
    // store nats connection client per actor
    actors: Arc<RwLock<HashMap<String, StorageClient>>>,
}

// use default implementations of provider message handlers
impl ProviderDispatch for S3BlobstoreProvider {}

impl S3BlobstoreProvider {
    async fn client(&self, ctx: &Context) -> RpcResult<StorageClient> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| RpcError::InvalidParameter("no actor in request".to_string()))?;
        // get read lock on actor-client hashmap
        let rd = self.actors.read().await;
        let client = rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        Ok(client.clone())
    }

    // s3 client: like client() but doesn't clone linkdefinition
    // (possibly a premature optimization!)
    async fn s3_client(&self, ctx: &Context) -> RpcResult<StorageClient> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| RpcError::InvalidParameter("no actor in request".to_string()))?;
        // get read lock on actor-client hashmap
        let rd = self.actors.read().await;
        let client = rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        Ok(StorageClient(client.0.clone(), None))
    }
}

/// Handle provider control commands
/// put_link (new actor link command), del_link (remove link command), and shutdown
#[async_trait]
impl ProviderHandler for S3BlobstoreProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        let config = StorageConfig::from_values(&ld.values)?;
        let link = StorageClient::new(config, Some(ld.clone())).await;

        let mut update_map = self.actors.write().await;
        update_map.insert(ld.actor_id.to_string(), link);

        Ok(true)
    }

    /// Handle notification that a link is dropped: close the connection
    async fn delete_link(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;
        if let Some(link) = aw.remove(actor_id) {
            // close and drop the connection
            let _ = link.close().await;
        }
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> Result<(), Infallible> {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
        for (_, link) in aw.drain() {
            // close and drop each connection
            let _ = link.close().await;
        }
        Ok(())
    }
}

/// Handle Blobstore methods that interact with S3
/// To simplify testing, the methods are also implemented for StorageClient,
#[async_trait]
impl Blobstore for S3BlobstoreProvider {
    async fn container_exists(&self, ctx: &Context, arg: &ContainerId) -> RpcResult<bool> {
        let client = self.s3_client(ctx).await?;
        client.container_exists(ctx, arg).await
    }

    async fn create_container(&self, ctx: &Context, arg: &ContainerId) -> RpcResult<()> {
        let client = self.s3_client(ctx).await?;
        client.create_container(ctx, arg).await
    }

    async fn get_container_info(
        &self,
        ctx: &Context,
        arg: &ContainerId,
    ) -> RpcResult<ContainerMetadata> {
        let client = self.s3_client(ctx).await?;
        client.get_container_info(ctx, arg).await
    }

    async fn list_containers(&self, ctx: &Context) -> RpcResult<ContainersInfo> {
        let client = self.s3_client(ctx).await?;
        client.list_containers(ctx).await
    }

    async fn remove_containers(&self, ctx: &Context, arg: &ContainerIds) -> RpcResult<MultiResult> {
        let client = self.s3_client(ctx).await?;
        client.remove_containers(ctx, arg).await
    }

    async fn object_exists(&self, ctx: &Context, arg: &ContainerObject) -> RpcResult<bool> {
        let client = self.s3_client(ctx).await?;
        client.object_exists(ctx, arg).await
    }

    async fn list_objects(
        &self,
        ctx: &Context,
        arg: &ListObjectsRequest,
    ) -> RpcResult<ListObjectsResponse> {
        let client = self.s3_client(ctx).await?;
        client.list_objects(ctx, arg).await
    }

    async fn remove_objects(
        &self,
        ctx: &Context,
        arg: &RemoveObjectsRequest,
    ) -> RpcResult<MultiResult> {
        let client = self.s3_client(ctx).await?;
        client.remove_objects(ctx, arg).await
    }

    async fn put_object(
        &self,
        ctx: &Context,
        arg: &PutObjectRequest,
    ) -> RpcResult<PutObjectResponse> {
        let client = self.s3_client(ctx).await?;
        client.put_object(ctx, arg).await
    }

    async fn get_object(
        &self,
        ctx: &Context,
        arg: &GetObjectRequest,
    ) -> RpcResult<GetObjectResponse> {
        // this call needs client() instead of s3_client for actor callbacks
        let client = self.client(ctx).await?;
        client.get_object(ctx, arg).await
    }

    async fn put_chunk(&self, ctx: &Context, arg: &PutChunkRequest) -> RpcResult<()> {
        let client = self.s3_client(ctx).await?;
        client.put_chunk(ctx, arg).await
    }
}
