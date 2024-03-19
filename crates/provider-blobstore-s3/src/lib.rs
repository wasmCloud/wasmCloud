//! blobstore-s3 capability provider
//!
//! This capability provider exposes [S3](https://aws.amazon.com/s3/)-compatible object storage
//! (AKA "blob store") as a [wasmcloud capability](https://wasmcloud.com/docs/concepts/capabilities) which
//! can be used by actors on your lattice.
//!

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Result};
use aws_sdk_s3::primitives::ByteStream;
use tokio::sync::RwLock;
use tracing::error;

use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait,
    wasmcloud_provider_sdk::{Context, LinkConfig, ProviderOperationResult},
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
    async fn client(&self, ctx: &Context) -> Result<StorageClient> {
        let source_id = ctx.actor.as_ref().context("no actor in request")?;

        let client = self
            .actors
            .read()
            .await
            .get(source_id)
            .with_context(|| format!("actor not linked:{}", source_id))?
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
    async fn receive_link_config_as_target(
        &self,
        link_config: impl LinkConfig,
    ) -> ProviderOperationResult<()> {
        let source_id = link_config.get_source_id();
        let config_values = link_config.get_config();

        // Build storage config
        let config = match StorageConfig::from_values(config_values) {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, %source_id, "failed to build storage config");
                return Err(anyhow!(e).context("failed to build source config").into());
            }
        };

        let link = StorageClient::new(config, config_values, source_id.into()).await;

        let mut update_map = self.actors.write().await;
        update_map.insert(source_id.to_string(), link);

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection
    async fn delete_link(&self, source_id: &str) -> ProviderOperationResult<()> {
        let mut aw = self.actors.write().await;
        if let Some(link) = aw.remove(source_id) {
            let _ = link.close().await;
        }
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> ProviderOperationResult<()> {
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
impl WasmcloudBlobstoreBlobstore for BlobstoreS3Provider {
    async fn container_exists(&self, ctx: Context, container_name: String) -> bool {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return false;
            }
        };
        client.container_exists(&ctx, &container_name).await
    }

    async fn create_container(&self, ctx: Context, container_name: ContainerId) -> () {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return;
            }
        };

        client.create_container(&ctx, &container_name).await
    }

    async fn get_container_info(
        &self,
        ctx: Context,
        container_name: ContainerId,
    ) -> ContainerMetadata {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return ContainerMetadata {
                    container_id: String::default(),
                    created_at: None,
                };
            }
        };

        client.get_container_info(&ctx, &container_name).await
    }

    async fn get_object_info(&self, ctx: Context, arg: ContainerObjectSelector) -> ObjectMetadata {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return ObjectMetadata {
                    container_id: String::default(),
                    content_encoding: None,
                    content_length: 0,
                    content_type: None,
                    last_modified: None,
                    object_id: String::default(),
                };
            }
        };

        client.get_object_info(&ctx, &arg).await
    }

    async fn list_containers(&self, ctx: Context) -> Vec<ContainerMetadata> {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return Vec::new();
            }
        };

        client.list_containers(&ctx).await
    }

    async fn remove_containers(
        &self,
        ctx: Context,
        container_names: Vec<String>,
    ) -> Vec<OperationResult> {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return Vec::new();
            }
        };

        client.remove_containers(&ctx, &container_names).await
    }

    async fn object_exists(&self, ctx: Context, selector: ContainerObjectSelector) -> bool {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return false;
            }
        };

        client.object_exists(&ctx, &selector).await
    }

    async fn list_objects(&self, ctx: Context, req: ListObjectsRequest) -> ListObjectsResponse {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return ListObjectsResponse {
                    continuation: None,
                    is_last: true,
                    objects: Vec::new(),
                };
            }
        };

        client.list_objects(&ctx, &req).await
    }

    async fn remove_objects(
        &self,
        ctx: Context,
        arg: RemoveObjectsRequest,
    ) -> Vec<OperationResult> {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return Vec::new();
            }
        };

        client.remove_objects(&ctx, &arg).await
    }

    async fn put_object(&self, ctx: Context, req: PutObjectRequest) -> PutObjectResponse {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return PutObjectResponse { stream_id: None };
            }
        };

        client.put_object(&ctx, &req).await
    }

    async fn get_object(&self, ctx: Context, req: GetObjectRequest) -> GetObjectResponse {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return GetObjectResponse {
                    content_encoding: None,
                    content_length: 0,
                    content_type: None,
                    error: Some("failed to read file".into()),
                    initial_chunk: None,
                    success: false,
                };
            }
        };

        client.get_object(&ctx, &req).await
    }

    async fn put_chunk(&self, ctx: Context, req: PutChunkRequest) -> () {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return;
            }
        };

        client.put_chunk(&ctx, &req).await
    }
}
