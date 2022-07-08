//! main process for blobstore-s3 provider
//! This is a thin wrapper around the blobstore-s3 library
//!
use blobstore_s3_lib::{
    wasmcloud_interface_blobstore::{
        Blobstore, BlobstoreReceiver, ContainerId, ContainerIds, ContainerMetadata,
        ContainerObject, ContainersInfo, GetObjectRequest, GetObjectResponse, ListObjectsRequest,
        ListObjectsResponse, MultiResult, ObjectMetadata, PutChunkRequest, PutObjectRequest,
        PutObjectResponse, RemoveObjectsRequest,
    },
    StorageClient, StorageConfig,
};
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::sync::RwLock;
use wasmbus_rpc::{core::LinkDefinition, provider::prelude::*};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut vars: Vec<(String, String)> = std::env::vars().collect();
    vars.sort_by(|v, other| String::cmp(&v.0, &other.0));
    for (var, value) in vars.iter() {
        println!("(stdout) blobstore-s3: env: {}={}", &var, &value);
    }
    // handle lattice control messages and forward rpc to the provider dispatch
    // returns when provider receives a shutdown control message
    provider_main(
        S3BlobstoreProvider::default(),
        Some("Blobstore S3 Provider".to_string()),
    )?;

    eprintln!("blobstore-s3 provider exiting");
    Ok(())
}

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
        let client = self.client(ctx).await?;
        client.container_exists(ctx, arg).await
    }

    async fn create_container(&self, ctx: &Context, arg: &ContainerId) -> RpcResult<()> {
        let client = self.client(ctx).await?;
        client.create_container(ctx, arg).await
    }

    async fn get_container_info(
        &self,
        ctx: &Context,
        arg: &ContainerId,
    ) -> RpcResult<ContainerMetadata> {
        let client = self.client(ctx).await?;
        client.get_container_info(ctx, arg).await
    }

    async fn get_object_info(
        &self,
        ctx: &Context,
        arg: &ContainerObject,
    ) -> RpcResult<ObjectMetadata> {
        let client = self.client(ctx).await?;
        client.get_object_info(ctx, arg).await
    }

    async fn list_containers(&self, ctx: &Context) -> RpcResult<ContainersInfo> {
        let client = self.client(ctx).await?;
        client.list_containers(ctx).await
    }

    async fn remove_containers(&self, ctx: &Context, arg: &ContainerIds) -> RpcResult<MultiResult> {
        let client = self.client(ctx).await?;
        client.remove_containers(ctx, arg).await
    }

    async fn object_exists(&self, ctx: &Context, arg: &ContainerObject) -> RpcResult<bool> {
        let client = self.client(ctx).await?;
        client.object_exists(ctx, arg).await
    }

    async fn list_objects(
        &self,
        ctx: &Context,
        arg: &ListObjectsRequest,
    ) -> RpcResult<ListObjectsResponse> {
        let client = self.client(ctx).await?;
        client.list_objects(ctx, arg).await
    }

    async fn remove_objects(
        &self,
        ctx: &Context,
        arg: &RemoveObjectsRequest,
    ) -> RpcResult<MultiResult> {
        let client = self.client(ctx).await?;
        client.remove_objects(ctx, arg).await
    }

    async fn put_object(
        &self,
        ctx: &Context,
        arg: &PutObjectRequest,
    ) -> RpcResult<PutObjectResponse> {
        let client = self.client(ctx).await?;
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
        let client = self.client(ctx).await?;
        client.put_chunk(ctx, arg).await
    }
}
