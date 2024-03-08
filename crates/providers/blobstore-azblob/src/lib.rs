use std::num::NonZeroU32;
use std::{
    collections::HashMap,
    sync::Arc,
};

use anyhow::Context as _;

use anyhow::Result;
use azure_storage_blobs::prelude::*;
use config::StorageConfig;
use futures::StreamExt;
use tokio::sync::RwLock;

use tracing::error;
use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait, wasmcloud_provider_sdk::core::LinkDefinition,
    wasmcloud_provider_sdk::Context,
};

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: BlobstoreAzblobProvider,
    contract: "wasmcloud:blobstore",
    wit_bindgen_cfg: "provider-blobstore"
});

mod config;

/// number of items to return in get_objects if max_items not specified
const DEFAULT_MAX_ITEMS: u32 = 1000;

/// maximum size of message (in bytes) that we'll return from azblob (500MB)
const DEFAULT_MAX_CHUNK_SIZE_BYTES: u64 = 500 * 1024 * 1024;

/// Blobstore Azblob provider
///
/// This struct will be the target of generated implementations (via wit-provider-bindgen)
/// for the blobstore provider WIT contract
#[derive(Default, Clone)]
pub struct BlobstoreAzblobProvider {
    /// Per-actor storage for NATS connection clients
    actors: Arc<RwLock<HashMap<String, BlobServiceClient>>>,
}

impl BlobstoreAzblobProvider {
    /// Retrieve the per-actor [`BlobServiceClient`] for a given link context
    async fn client(&self, ctx: &Context) -> Result<BlobServiceClient> {
        let actor_id = ctx.actor.as_ref().context("no actor in request")?;

        let client = self
            .actors
            .read()
            .await
            .get(actor_id)
            .with_context(|| format!("actor not linked:{}", actor_id))?
            .clone();
        Ok(client)
    }
}

/// Handle provider control commands
/// put_link (new actor link command), del_link (remove link command), and shutdown
#[async_trait]
impl WasmcloudCapabilityProvider for BlobstoreAzblobProvider {
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        let config =
            match StorageConfig::from_values(&HashMap::from_iter(ld.values.iter().cloned())) {
                Ok(v) => v,
                Err(e) => {
                    error!(error = %e, actor_id = %ld.actor_id, "failed to read storage config");
                    return false;
                }
            };
        let link =
            BlobServiceClient::builder(config.storage_account.clone(), config.configure_az())
                .blob_service_client();

        let mut update_map = self.actors.write().await;
        update_map.insert(ld.actor_id.to_string(), link);

        true
    }

    /// Handle notification that a link is dropped: close the connection
    async fn delete_link(&self, actor_id: &str) {
        self.actors.write().await.remove(actor_id);
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) {
        self.actors.write().await.drain();
    }
}

/// Handle Blobstore methods that interact with Azblob
/// To simplify testing, the methods are also implemented for StorageClient,
#[async_trait]
impl WasmcloudBlobstoreBlobstore for BlobstoreAzblobProvider {
    async fn container_exists(&self, ctx: Context, container_name: String) -> bool {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return false;
            }
        };
        match client.container_client(container_name).exists().await {
            Ok(res) => res,
            Err(e) => {
                error!(error = %e, "failed to check container existence");
                false
            }
        }
    }

    async fn create_container(&self, ctx: Context, container_name: ContainerId) -> () {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return;
            }
        };
        match client.container_client(container_name).create().await {
            Ok(_) => (),
            Err(e) => error!(error = %e, "failed to create container"),
        }
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

        match client
            .container_client(container_name)
            .get_properties()
            .await
        {
            Ok(res) => ContainerMetadata {
                container_id: res.container.name,
                // TODO: no date?
                created_at: None,
            },
            Err(e) => {
                error!(error = %e, "failed to get container info");
                ContainerMetadata {
                    container_id: String::default(),
                    created_at: None,
                }
            }
        }
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

        match client
            .container_client(&arg.container_id)
            .blob_client(&arg.object_id)
            .get_properties()
            .await
        {
            Ok(res) => {
                let blob = res.blob;
                ObjectMetadata {
                    container_id: arg.container_id,
                    content_encoding: blob.properties.content_encoding,
                    content_length: blob.properties.content_length,
                    content_type: Some(blob.properties.content_type),
                    last_modified: Some(Timestamp::from(blob.properties.last_modified)),
                    object_id: arg.object_id,
                }
            }
            Err(se) => {
                error!(error = %se, "failed to get object info");
                ObjectMetadata {
                    container_id: String::default(),
                    content_encoding: None,
                    content_length: 0,
                    content_type: None,
                    last_modified: None,
                    object_id: String::default(),
                }
            }
        }
    }

    async fn list_containers(&self, ctx: Context) -> Vec<ContainerMetadata> {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return Vec::new();
            }
        };

        let mut stream = client.list_containers().into_stream();
        let mut all_containers = Vec::new();

        while let Some(response) = stream.next().await {
            match response {
                Ok(res) => {
                    res.containers.iter().for_each(|c| {
                        all_containers.push(ContainerMetadata {
                            container_id: c.name.clone(),
                            // TODO: no creation date?
                            created_at: None,
                        });
                    });
                }
                Err(e) => {
                    error!(error = %e, "failed to list containers");
                    return Vec::new();
                }
            };
        }

        all_containers
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

        let mut results = Vec::new();
        for container_name in container_names.iter() {
            match client
                .container_client(container_name)
                .delete()
                .into_future()
                .await
            {
                Ok(_) => results.push(OperationResult {
                    key: container_name.to_string(),
                    error: None,
                    success: true,
                }),
                Err(e) => {
                    error!(error = %e, "failed to delete container");
                    results.push(OperationResult {
                        key: container_name.to_string(),
                        error: Some(e.to_string()),
                        success: false,
                    });
                }
            }
        }

        results
    }

    async fn object_exists(&self, ctx: Context, arg: ContainerObjectSelector) -> bool {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return false;
            }
        };

        match client
            .container_client(&arg.container_id)
            .blob_client(&arg.object_id)
            .exists()
            .await
        {
            Ok(res) => res,
            Err(e) => {
                error!(error = %e, "failed to check object existence");
                false
            }
        }
    }

    async fn list_objects(&self, ctx: Context, req: ListObjectsRequest) -> ListObjectsResponse {
        //
        // TODO: az-blob-rust-sdk does not support continuation token
        //

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

        let mut list_blob = client.container_client(req.container_id).list_blobs();

        if let Some(max_items) = req.max_items {
            if max_items > i32::MAX as u32 {
                error!("max items too large");
                return ListObjectsResponse {
                    continuation: None,
                    is_last: true,
                    objects: vec![],
                };
            }
            list_blob = list_blob.max_results(NonZeroU32::new(max_items).unwrap());
        } else {
            list_blob = list_blob.max_results(NonZeroU32::new(DEFAULT_MAX_ITEMS).unwrap());
        }

        let mut stream = list_blob.into_stream();

        let mut all_blobs = Vec::new();

        while let Some(response) = stream.next().await {
            match response {
                Ok(res) => {
                    res.blobs.blobs().for_each(|c| {
                        all_blobs.push(ObjectMetadata {
                            container_id: c.name.clone(),
                            last_modified: Some(Timestamp::from(c.properties.last_modified)),
                            object_id: c.name.to_string(),
                            content_length: c.properties.content_length,
                            content_encoding: c.properties.content_encoding.clone(),
                            content_type: Some(c.properties.content_type.to_string()),
                        });
                    });
                }
                Err(e) => {
                    error!(error = %e, "failed to list containers");
                }
            };
        }

        ListObjectsResponse {
            continuation: None,
            is_last: true,
            objects: all_blobs,
        }
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

        let container_client = client.container_client(&arg.container_id);
        let mut results = Vec::new();
        for obj in arg.objects.iter() {
            let blob_client = container_client.blob_client(obj);
            match blob_client.delete().into_future().await {
                Ok(_) => {}
                Err(e) => {
                    error!(error = %e, "failed to delete object");
                    results.push(OperationResult {
                        key: obj.clone(),
                        error: Some(e.to_string()),
                        success: false,
                    });
                }
            }
        }

        if !results.is_empty() {
            error!(
                "delete_objects returned {}/{} errors",
                results.len(),
                arg.objects.len()
            );
        }
        results
    }

    async fn put_object(&self, ctx: Context, req: PutObjectRequest) -> PutObjectResponse {
        let client = match self.client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return PutObjectResponse { stream_id: None };
            }
        };

        let container_client = client.container_client(&req.chunk.container_id);
        if !req.chunk.is_last {
            error!("put_object for multi-part upload: not implemented!");
            return PutObjectResponse { stream_id: None };
        }
        if req.chunk.offset != 0 {
            error!("put_object with initial offset non-zero: not implemented!");
            return PutObjectResponse { stream_id: None };
        }
        if req.chunk.bytes.is_empty() {
            error!("put_object with zero bytes");
            return PutObjectResponse { stream_id: None };
        }

        let bytes = req.chunk.bytes.to_owned();
        let blob_client = container_client.blob_client(&req.chunk.object_id);
        let mut builder = blob_client.put_block_blob(bytes);

        if let Some(content_type) = req.content_type {
            builder = builder.content_type(content_type);
        }

        if let Some(content_encoding) = req.content_encoding {
            builder = builder.content_encoding(content_encoding);
        }

        match builder.await {
            Ok(_) => PutObjectResponse { stream_id: None },
            Err(e) => {
                error!(error = %e, "failed to put object");
                PutObjectResponse { stream_id: None }
            }
        }
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

        let mut stream = client
            .container_client(&req.container_id)
            .blob_client(&req.object_id)
            .get()
            .chunk_size(DEFAULT_MAX_CHUNK_SIZE_BYTES)
            .into_stream();
        let mut result = vec![];
        // The stream is composed of individual calls to the get blob endpoint
        while let Some(value) = stream.next().await {
            match value {
                Ok(value) => {
                    let data = value.data.collect().await.unwrap();
                    result.extend(&data);
                }
                Err(e) => {
                    error!(error = %e, "failed to get object");
                    return GetObjectResponse {
                        content_encoding: None,
                        content_length: 0,
                        content_type: None,
                        error: Some("failed to read file".into()),
                        initial_chunk: None,
                        success: false,
                    };
                }
            }
        }

        GetObjectResponse {
            content_encoding: None,
            content_length: result.len() as u64,
            content_type: None,
            error: None,
            initial_chunk: Some(Chunk {
                bytes: result,
                is_last: true,
                offset: 0,
                object_id: req.object_id.clone(),
                container_id: req.container_id.clone(),
            }),
            success: true,
        }
    }

    async fn put_chunk(&self, _ctx: Context, _req: PutChunkRequest) -> () {
        error!("put_chunk is unimplemented");
    }
}

impl From<time::OffsetDateTime> for Timestamp {
    fn from(dt: time::OffsetDateTime) -> Self {
        Timestamp {
            sec: dt.unix_timestamp() as u64,
            nsec: dt.nanosecond(),
        }
    }
}
