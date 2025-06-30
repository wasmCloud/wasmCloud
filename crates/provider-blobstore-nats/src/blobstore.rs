//! NATS implementation for `wrpc:blobstore/blobstore@0.2.0` interface.

#![allow(clippy::type_complexity)]
use anyhow::{Context as _, Result};
use bytes::Bytes;
use core::future::Future;
use core::pin::Pin;
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{info, instrument};
use wasmcloud_provider_sdk::{propagate_trace_for_ctx, Context};

// Import the wrpc interface bindings
use wrpc_interface_blobstore::bindings;

impl bindings::exports::wrpc::blobstore::blobstore::Handler<Option<Context>>
    for crate::NatsBlobstoreProvider
{
    // Create a new NATS Blobstore Container
    #[instrument(level = "debug", skip(self))]
    async fn create_container(
        &self,
        context: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Create a new NATS Blobstore Container, with provided storage configuration
            let blobstore = self.get_blobstore(context).await.context(
                "failed to get NATS Blobstore connection and container storage configuration",
            )?;

            // Create a NATS Blobstore Container configuration
            let container_config = async_nats::jetstream::object_store::Config {
                bucket: name,
                description: Some("NATS Blobstore".to_string()),
                max_age: blobstore.storage_config.max_age,
                max_bytes: blobstore.storage_config.max_bytes,
                storage: blobstore.storage_config.storage_type.0,
                num_replicas: blobstore.storage_config.num_replicas,
                compression: blobstore.storage_config.compression,
                placement: None,
            };

            // Create a NATS Blobstore Container
            blobstore
                .jetstream
                .create_object_store(container_config)
                .await
                .context("failed to create NATS Blobstore Container")
                .map(|_| ())
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    // Get metadata of an existing NATS Blobstore Container
    #[instrument(level = "trace", skip(self))]
    async fn get_container_info(
        &self,
        context: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<bindings::wrpc::blobstore::types::ContainerMetadata, String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Retrieve the blobstore connection
            let blobstore = self
                .get_blobstore(context)
                .await
                .context("failed to get NATS Blobstore connection")?;

            // Attempt to get the container metadata
            let _container = blobstore
                .jetstream
                .get_object_store(name)
                .await
                .context("failed to get container info")?;

            // Construct and return the container metadata
            let metadata = bindings::wrpc::blobstore::types::ContainerMetadata {
                created_at: 0u64, // Unix epoch as a placeholder
            };
            Ok(metadata)
        }
        .await
        .map_err(|err: anyhow::Error| err.to_string()))
    }

    // Check if a NATS Blobstore Container exists
    #[instrument(level = "debug", skip(self))]
    async fn container_exists(
        &self,
        context: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<bool, String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Retrieve the NATS blobstore connection
            let blobstore = self
                .get_blobstore(context)
                .await
                .map_err(|e| e.to_string())?;

            // Check if the container exists
            match blobstore.jetstream.get_object_store(&name).await {
                Ok(_) => Ok(true),
                Err(e)
                    if matches!(
                        e.kind(),
                        async_nats::jetstream::context::ObjectStoreErrorKind::GetStore
                    ) =>
                {
                    Ok(false)
                }
                Err(e) => Err(format!("failed to check container existence: {e}")),
            }
        }
        .await
        .map_err(|err| err.to_string()))
    }

    /// Retrieve data from an object in the specified NATS blobstore Container
    /// Optionally specify start and end byte offsets for partial reads
    #[instrument(level = "debug", skip(self))]
    async fn get_container_data(
        &self,
        context: Option<Context>,
        id: bindings::wrpc::blobstore::types::ObjectId,
        start: u64,
        end: u64,
    ) -> anyhow::Result<
        Result<
            (
                Pin<Box<dyn Stream<Item = Bytes> + Send>>,
                Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
            ),
            String,
        >,
    > {
        use tokio::io::AsyncReadExt; // Import the trait to use `read`

        Ok(async {
            propagate_trace_for_ctx!(context);

            // Retrieve the NATS blobstore connection
            let blobstore = self
                .get_blobstore(context)
                .await
                .context("failed to get NATS Blobstore connection")?;

            // Get the container (object store) for the specified container name
            let container = blobstore
                .jetstream
                .get_object_store(&id.container)
                .await
                .context("failed to get container")?;

            // Retrieve the object data as a stream
            let mut object = container
                .get(&id.object)
                .await
                .context("failed to get object data")?;

            // Create a channel to stream the data
            let (tx, rx) = mpsc::channel(16);
            anyhow::Ok((
                Box::pin(ReceiverStream::new(rx)) as Pin<Box<dyn Stream<Item = _> + Send>>,
                Box::pin(async move {
                    async move {
                        // Stream the object data in chunks of 1024 bytes
                        let mut buffer = vec![0; 1024];
                        while let Ok(bytes_read) = object.read(&mut buffer).await {
                            if bytes_read == 0 {
                                break;
                            }
                            let chunk = Bytes::copy_from_slice(&buffer[..bytes_read]);
                            tx.send(chunk).await.context("stream receiver closed")?;
                        }
                        anyhow::Ok(())
                    }
                    .await
                    .map_err(|err| format!("{err:#}"))
                }) as Pin<Box<dyn Future<Output = _> + Send>>,
            ))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    // Create or replace an object with the data blob in the specified NATS blobstore Container
    #[instrument(level = "debug", skip(self, data))]
    async fn write_container_data(
        &self,
        context: Option<Context>,
        id: bindings::wrpc::blobstore::types::ObjectId,
        data: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    ) -> anyhow::Result<Result<Pin<Box<dyn Future<Output = Result<(), String>> + Send>>, String>>
    {
        Ok(async {
            propagate_trace_for_ctx!(context);

            let blobstore = self
                .get_blobstore(context)
                .await
                .context("failed to get NATS Blobstore connection")?;

            let container = blobstore
                .jetstream
                .get_object_store(&id.container)
                .await
                .context("failed to get container")?;

            let metadata = async_nats::jetstream::object_store::ObjectMetadata {
                name: id.object.clone(),
                description: Some("NATS WASI Blobstore Object".to_string()),
                chunk_size: Some(256 * 1024), // 256KB chunks
                headers: None,                // No custom headers
                metadata: HashMap::new(),     // Empty metadata map
            };

            let result: Result<(), String> = async move {
                let data = data.map(Ok::<_, std::io::Error>);
                let mut reader = tokio_util::io::StreamReader::new(data);

                // Get timeout from config, defaulting to 30 seconds if not set
                let timeout = Duration::from_secs(self.default_config.max_write_wait.unwrap_or(30));

                tokio::time::timeout(timeout, container.put(metadata, &mut reader))
                    .await
                    .context("operation timed out")
                    .map_err(|e| e.to_string())?
                    .context("failed to write container data")
                    .map_err(|e| e.to_string())?;

                Ok(())
            }
            .await;

            Ok(Box::pin(async move { result })
                as Pin<Box<dyn Future<Output = Result<(), String>> + Send>>)
        }
        .await
        .map_err(|err: anyhow::Error| format!("{err:#}")))
    }

    /// Helper function to list all objects in a NATS blobstore container.
    /// This ensures consistent implementation across all functions that need to list objects.
    /// Delegates to the core implementation in blobstore.rs.
    #[instrument(level = "debug", skip_all)]
    async fn list_container_objects(
        &self,
        context: Option<Context>,
        name: String,
        _offset: Option<u64>,
        _limit: Option<u64>,
    ) -> anyhow::Result<
        Result<
            (
                Pin<Box<dyn Stream<Item = Vec<String>> + Send>>,
                Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
            ),
            String,
        >,
    > {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Retrieve the NATS blobstore connection
            let blobstore = self
                .get_blobstore(context)
                .await
                .context("failed to get NATS Blobstore connection")?;

            // Get the container (object store) for the specified container name
            let container = blobstore
                .jetstream
                .get_object_store(&name)
                .await
                .context("failed to get container")?;

            // Get the list of objects in the container
            let mut objects = container
                .list()
                .await
                .context("failed to list container objects")?;

            // Create a channel to stream the data
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            anyhow::Ok((
                Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
                    as Pin<Box<dyn Stream<Item = Vec<String>> + Send>>,
                Box::pin(async move {
                    while let Some(object) = objects.next().await {
                        let object = object.map_err(|e| format!("{e:#}"))?;
                        tx.send(vec![object.name])
                            .await
                            .map_err(|e| format!("{e:#}"))?;
                    }
                    Ok(())
                }) as Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
            ))
        }
        .await
        .map_err(|err: anyhow::Error| format!("{err:#}")))
    }

    // Remove all objects within the NATS blobstore Container, leaving the container empty.
    #[instrument(level = "debug", skip(self))]
    async fn clear_container(
        &self,
        context: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // List all objects in the container
            let (mut objects_stream, _) = self
                .list_container_objects(context.clone(), name.clone(), None, None)
                .await
                .context("failed to list container objects")
                .map(|r| r.map_err(|e| anyhow::anyhow!(e)))??;

            // Collect ALL objects from the stream into a single vector
            let mut all_objects = Vec::new();
            while let Some(batch) = objects_stream.next().await {
                all_objects.extend(batch);
            }

            // Delete all objects in the container
            self.delete_objects(context, name, all_objects)
                .await
                .context("failed to delete objects")
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!(e))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    // Delete an existing NATS Blobstore Container
    #[instrument(level = "debug", skip(self))]
    async fn delete_container(
        &self,
        context: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Delete an existing NATS Blobstore
            let blobstore = self
                .get_blobstore(context)
                .await
                .context("failed to get NATS Blobstore connection")?;

            // Delete the container
            blobstore
                .jetstream
                .delete_object_store(name)
                .await
                .context("failed to delete NATS Blobstore Container")
                .map(|_| ())
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    // Check if an object exists in the specified NATS Blobstore Container
    #[instrument(level = "debug", skip(self))]
    async fn has_object(
        &self,
        context: Option<Context>,
        id: bindings::wrpc::blobstore::types::ObjectId,
    ) -> anyhow::Result<Result<bool, String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Retrieve the NATS blobstore connection
            let blobstore = self
                .get_blobstore(context)
                .await
                .context("failed to get NATS Blobstore connection")?;

            // Get the container (object store) for the specified container name
            let container = blobstore
                .jetstream
                .get_object_store(&id.container)
                .await
                .context("failed to get container")?;

            // Check if the object exists
            container
                .info(id.object)
                .await
                .context("failed to get object info")
                .map(|_| true)
                .map_err(|e| anyhow::anyhow!(e))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    // Get metadata of an object in the specified NATS blobstore Container
    #[instrument(level = "debug", skip(self))]
    async fn get_object_info(
        &self,
        context: Option<Context>,
        id: bindings::wrpc::blobstore::types::ObjectId,
    ) -> anyhow::Result<Result<bindings::wrpc::blobstore::types::ObjectMetadata, String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Retrieve the NATS blobstore connection
            let blobstore = self
                .get_blobstore(context)
                .await
                .context("failed to get NATS Blobstore connection")?;

            // Get the container (object store) for the specified container name
            let container = blobstore
                .jetstream
                .get_object_store(&id.container)
                .await
                .context("failed to get container")?;

            // Get the object info
            container
                .info(id.object)
                .await
                .context("failed to get object info")
                .map(
                    |object_info| bindings::wrpc::blobstore::types::ObjectMetadata {
                        // NATS doesn't store the object creation time, so always return the Unix epoch
                        created_at: 0,
                        size: object_info.size as u64,
                    },
                )
                .map_err(|e| anyhow::anyhow!(e))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    // Copy an object from one to the same (different key name, or revision number update), or another NATS Blobstore Container
    #[instrument(level = "debug", skip(self))]
    async fn copy_object(
        &self,
        context: Option<Context>,
        source: bindings::wrpc::blobstore::types::ObjectId,
        destination: bindings::wrpc::blobstore::types::ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Skip the copy if source and destination are the same
            if source.container == destination.container && source.object == destination.object {
                info!(
                    "skipping copying object '{}' to itself in container '{}'",
                    source.object, source.container
                );
                return Ok(());
            }

            // Retrieve the NATS blobstore connection
            let blobstore = self
                .get_blobstore(context)
                .await
                .context("failed to get NATS Blobstore connection")?;

            // Get the source container (object store)
            let src_container = blobstore
                .jetstream
                .get_object_store(&source.container)
                .await
                .context("failed to open source NATS Blobstore Container")?;

            // Get the destination container (object store)
            let dst_container = blobstore
                .jetstream
                .get_object_store(&destination.container)
                .await
                .context("failed to open destination NATS Blobstore Container")?;

            // Get the source object
            let mut src_object = src_container
                .get(source.object.clone())
                .await
                .context("failed to read object from source container")?;

            // Prepare metadata for the destination object
            let metadata = async_nats::jetstream::object_store::ObjectMetadata {
                name: destination.object.clone(),
                description: src_object.info.description.clone(),
                chunk_size: Some(src_object.info.chunks),
                headers: None,            // No custom headers
                metadata: HashMap::new(), // Empty metadata map
            };

            // Put the object into the destination container
            dst_container
                .put(metadata, &mut src_object)
                .await
                .context("failed to copy object to destination container")
                .map(|_| ())
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    // Move an object from one to the same (different key name, or revision number update), or another NATS Blobstore Container
    #[instrument(level = "debug", skip(self))]
    async fn move_object(
        &self,
        context: Option<Context>,
        source: bindings::wrpc::blobstore::types::ObjectId,
        destination: bindings::wrpc::blobstore::types::ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Skip the move if source and destination are the same
            if source.container == destination.container && source.object == destination.object {
                info!(
                    "skipping moving object '{}' to itself in container '{}'",
                    source.object, source.container
                );
                return Ok(());
            }

            // Retrieve the NATS blobstore connection
            let blobstore = self
                .get_blobstore(context)
                .await
                .context("failed to get NATS Blobstore connection")?;

            // Get the source container (object store)
            let src_container = blobstore
                .jetstream
                .get_object_store(&source.container)
                .await
                .context("failed to open source NATS Blobstore Container")?;

            // Get the destination container (object store)
            let dst_container = blobstore
                .jetstream
                .get_object_store(&destination.container)
                .await
                .context("failed to open destination NATS Blobstore Container")?;

            // Get the source object
            let mut src_object = src_container
                .get(source.object.clone())
                .await
                .context("failed to read object from source container")?;

            // Prepare metadata for the destination object
            let metadata = async_nats::jetstream::object_store::ObjectMetadata {
                name: destination.object.clone(),
                description: src_object.info.description.clone(),
                chunk_size: Some(src_object.info.chunks),
                headers: None,            // No custom headers
                metadata: HashMap::new(), // Empty metadata map
            };

            // Put the object into the destination container
            dst_container
                .put(metadata, &mut src_object)
                .await
                .context("failed to move object to destination container")?;

            // Delete the source object
            src_container
                .delete(source.object.clone())
                .await
                .context("failed to delete object from source container")
                .map(|_| ())
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    // Delete an object in the specified NATS Blobstore Container
    #[instrument(level = "debug", skip(self))]
    async fn delete_object(
        &self,
        context: Option<Context>,
        id: bindings::wrpc::blobstore::types::ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Retrieve the NATS blobstore connection
            let blobstore = self
                .get_blobstore(context)
                .await
                .map_err(|e| e.to_string())?;

            // Get the container (object store) for the specified container name
            let container = blobstore
                .jetstream
                .get_object_store(&id.container)
                .await
                .map_err(|e| e.to_string())?;
            // Delete the object
            let result: Result<(), String> =
                container.delete(id.object).await.map_err(|e| e.to_string());

            result
        }
        .await
        .map_err(|err: String| format!("{err:#}")))
    }

    // Delete multiple objects in the specified NATS Blobstore Container
    // Objects are deleted concurrently in batches for improved performance
    #[instrument(level = "trace", skip(self))]
    async fn delete_objects(
        &self,
        context: Option<Context>,
        name: String,
        objects: Vec<String>,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(context);

            // Create deletion tasks in batches of 50 to prevent overwhelming the server
            const BATCH_SIZE: usize = 50;
            let mut handles = Vec::with_capacity(objects.len());

            // Process objects in chunks to maintain reasonable resource usage
            for chunk in objects.chunks(BATCH_SIZE) {
                let mut chunk_handles = chunk
                    .iter()
                    .map(|object| {
                        let ctx = context.clone();
                        let container = name.clone();
                        let object = object.clone();
                        let this = self.clone();

                        tokio::spawn(async move {
                            this.delete_object(
                                ctx,
                                bindings::wrpc::blobstore::types::ObjectId { container, object },
                            )
                            .await
                        })
                    })
                    .collect::<Vec<_>>();
                handles.append(&mut chunk_handles);
            }

            // Wait for all deletion tasks to complete
            let results = futures::future::join_all(handles).await;

            // Process results and collect any errors
            let errors: Vec<String> = results
                .into_iter()
                .filter_map(|r| match r {
                    Ok(Ok(Ok(()))) => None,                               // Successful deletion
                    Ok(Ok(Err(e))) => Some(e),                            // Operation error
                    Ok(Err(e)) => Some(format!("Provider error: {e:#}")), // Provider error
                    Err(e) => Some(format!("Task join error: {e:#}")),    // Task execution error
                })
                .collect();

            if errors.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "Failed to delete some objects: {}",
                    errors.join("; ")
                ))
            }
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }
}
