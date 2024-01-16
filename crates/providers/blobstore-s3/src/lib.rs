//! blobstore-s3 capability provider
//!
//! This capability provider exposes [S3](https://aws.amazon.com/s3/)-compatible object storage
//! (AKA "blob store") as a [wasmcloud capability](https://wasmcloud.com/docs/concepts/capabilities) which
//! can be used by actors on your lattice.
//!

use std::collections::HashMap;
use std::sync::Arc;

use aws_sdk_s3::primitives::ByteStream;
use tokio::sync::RwLock;
use tracing::error;

use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait,
    wasmcloud_provider_sdk::core::LinkDefinition,
    wasmcloud_provider_sdk::error::{ProviderInvocationError, ProviderInvocationResult},
    wasmcloud_provider_sdk::Context,
};

mod config;
pub use config::StorageConfig;

mod client;
pub use client::StorageClient;

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: BlobstoreS3Provider,
    contract: "wasmcloud:blobstore",
    wit_bindgen_cfg: "provider"
});

/// Blobstore S3 provider
///
/// This struct will be the target of generated implementations (via wit-provider-bindgen)
/// for the blobstore provider WIT contract
#[derive(Default, Clone)]
pub struct BlobstoreS3Provider {
    /// Per-actor storage for NATS connection clients
    actors: Arc<RwLock<HashMap<String, StorageClient>>>,
}

impl BlobstoreS3Provider {
    /// Retrieve the per-actor [`StorageClient`] for a given link context
    async fn client(&self, ctx: &Context) -> ProviderInvocationResult<StorageClient> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| ProviderInvocationError::Provider("no actor in request".to_string()))?;
        let client = self
            .actors
            .read()
            .await
            .get(actor_id)
            .ok_or_else(|| {
                ProviderInvocationError::Provider(format!("actor not linked:{}", actor_id))
            })?
            .clone();
        Ok(client)
    }
}

/// Handle provider control commands
/// put_link (new actor link command), del_link (remove link command), and shutdown
#[async_trait]
impl WasmcloudCapabilityProvider for BlobstoreS3Provider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        let config =
            match StorageConfig::from_values(&HashMap::from_iter(ld.values.iter().cloned())) {
                Ok(v) => v,
                Err(e) => {
                    error!(error = %e, actor_id = %ld.actor_id, "failed to read storage config");
                    return false;
                }
            };
        let link = StorageClient::new(config, ld.to_owned()).await;

        let mut update_map = self.actors.write().await;
        update_map.insert(ld.actor_id.to_string(), link);

        true
    }

    /// Handle notification that a link is dropped: close the connection
    async fn delete_link(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;
        if let Some(link) = aw.remove(actor_id) {
            let _ = link.close().await;
        }
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
        for (_, link) in aw.drain() {
            // close and drop each connection
            let _ = link.close().await;
        }
    }
}

/// Handle Blobstore methods that interact with S3
/// To simplify testing, the methods are also implemented for StorageClient,
#[async_trait]
impl WasmcloudBlobstoreBlobstore for BlobstoreS3Provider {
    async fn container_exists(
        &self,
        ctx: Context,
        container_name: String,
    ) -> ProviderInvocationResult<bool> {
        self.client(&ctx)
            .await?
            .container_exists(&ctx, &container_name)
            .await
    }

    async fn create_container(
        &self,
        ctx: Context,
        container_name: ContainerId,
    ) -> ProviderInvocationResult<()> {
        self.client(&ctx)
            .await?
            .create_container(&ctx, &container_name)
            .await
    }

    async fn get_container_info(
        &self,
        ctx: Context,
        container_name: ContainerId,
    ) -> ProviderInvocationResult<ContainerMetadata> {
        self.client(&ctx)
            .await?
            .get_container_info(&ctx, &container_name)
            .await
    }

    async fn get_object_info(
        &self,
        ctx: Context,
        arg: ContainerObjectSelector,
    ) -> ProviderInvocationResult<ObjectMetadata> {
        self.client(&ctx).await?.get_object_info(&ctx, &arg).await
    }

    async fn list_containers(
        &self,
        ctx: Context,
    ) -> ProviderInvocationResult<Vec<ContainerMetadata>> {
        self.client(&ctx).await?.list_containers(&ctx).await
    }

    async fn remove_containers(
        &self,
        ctx: Context,
        container_names: Vec<String>,
    ) -> ProviderInvocationResult<Vec<OperationResult>> {
        self.client(&ctx)
            .await?
            .remove_containers(&ctx, &container_names)
            .await
    }

    async fn object_exists(
        &self,
        ctx: Context,
        selector: ContainerObjectSelector,
    ) -> ProviderInvocationResult<bool> {
        self.client(&ctx)
            .await?
            .object_exists(&ctx, &selector)
            .await
    }

    async fn list_objects(
        &self,
        ctx: Context,
        req: ListObjectsRequest,
    ) -> ProviderInvocationResult<ListObjectsResponse> {
        self.client(&ctx).await?.list_objects(&ctx, &req).await
    }

    async fn remove_objects(
        &self,
        ctx: Context,
        arg: RemoveObjectsRequest,
    ) -> ProviderInvocationResult<Vec<OperationResult>> {
        self.client(&ctx).await?.remove_objects(&ctx, &arg).await
    }

    async fn put_object(
        &self,
        ctx: Context,
        req: PutObjectRequest,
    ) -> ProviderInvocationResult<PutObjectResponse> {
        self.client(&ctx).await?.put_object(&ctx, &req).await
    }

    async fn get_object(
        &self,
        ctx: Context,
        req: GetObjectRequest,
    ) -> ProviderInvocationResult<GetObjectResponse> {
        self.client(&ctx).await?.get_object(&ctx, &req).await
    }

    async fn put_chunk(&self, ctx: Context, req: PutChunkRequest) -> ProviderInvocationResult<()> {
        self.client(&ctx).await?.put_chunk(&ctx, &req).await
    }
}
