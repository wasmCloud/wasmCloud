#![allow(clippy::type_complexity)]

use core::pin::{pin, Pin};

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use azure_storage_blobs::prelude::*;
use bindings::wrpc::blobstore::types::{ContainerMetadata, ObjectId, ObjectMetadata};
use bytes::{Bytes, BytesMut};
use futures::{stream, Stream, StreamExt as _, TryStreamExt as _};
use tokio::sync::{mpsc, RwLock};
use tokio::{select, spawn};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, instrument, warn};
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, propagate_trace_for_ctx, run_provider, Context,
    LinkConfig, Provider,
};

use config::StorageConfig;

mod config;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wasi:blobstore/types@0.2.0-draft": generate,
            "wasi:io/error@0.2.0": generate,
            "wasi:io/poll@0.2.0": generate,
            "wasi:io/streams@0.2.0": generate,
            "wrpc:blobstore/blobstore@0.1.0": generate,
            "wrpc:blobstore/types@0.1.0": generate,
        }
    });
}

/// Blobstore Azblob provider
///
/// This struct will be the target of generated implementations (via wit-provider-bindgen)
/// for the blobstore provider WIT contract
#[derive(Default, Clone)]
pub struct BlobstoreAzblobProvider {
    /// Per-config storage for Azure connection clients
    config: Arc<RwLock<HashMap<String, BlobServiceClient>>>,
}

pub async fn run() -> anyhow::Result<()> {
    BlobstoreAzblobProvider::run().await
}

/// Handle provider control commands
/// put_link (new component link command), del_link (remove link command), and shutdown
impl Provider for BlobstoreAzblobProvider {
    #[instrument(level = "info", skip_all)]
    async fn receive_link_config_as_target(
        &self,
        link_config: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let config = match StorageConfig::from_values(link_config.config) {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, source_id = %link_config.source_id, "failed to read storage config");
                return Err(e);
            }
        };
        let link =
            BlobServiceClient::builder(config.storage_account.clone(), config.configure_az())
                .blob_service_client();

        let mut update_map = self.config.write().await;
        update_map.insert(link_config.source_id.to_string(), link);

        Ok(())
    }

    async fn delete_link(&self, source_id: &str) -> anyhow::Result<()> {
        self.config.write().await.remove(source_id);
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.config.write().await.drain();
        Ok(())
    }
}

impl BlobstoreAzblobProvider {
    pub async fn run() -> anyhow::Result<()> {
        initialize_observability!(
            "blobstore-azure-provider",
            std::env::var_os("PROVIDER_BLOBSTORE_AZURE_FLAMEGRAPH_PATH")
        );

        let provider = Self::default();
        let shutdown = run_provider(provider.clone(), "blobstore-azure-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        let invocations = bindings::serve(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
        )
        .await
        .context("failed to serve exports")?;
        let mut invocations = stream::select_all(invocations.into_iter().map(
            |(instance, name, invocations)| {
                invocations
                    .try_buffer_unordered(256)
                    .map(move |res| (instance, name, res))
            },
        ));
        let mut shutdown = pin!(shutdown);
        loop {
            select! {
                Some((instance, name, res)) = invocations.next() => {
                    if let Err(err) = res {
                        warn!(?err, instance, name, "failed to serve invocation");
                    } else {
                        debug!(instance, name, "successfully served invocation");
                    }
                },
                () = &mut shutdown => {
                    return Ok(())
                }
            }
        }
    }

    async fn get_config(&self, context: Option<&Context>) -> anyhow::Result<BlobServiceClient> {
        if let Some(source_id) = context.and_then(|Context { component, .. }| component.as_ref()) {
            self.config
                .read()
                .await
                .get(source_id)
                .with_context(|| format!("failed to lookup {source_id} configuration"))
                .cloned()
        } else {
            bail!(
                "failed to lookup source of invocation, could not construct Azure blobstore client"
            )
        }
    }
}

impl bindings::exports::wrpc::blobstore::blobstore::Handler<Option<Context>>
    for BlobstoreAzblobProvider
{
    #[instrument(level = "trace", skip(self))]
    async fn clear_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let stream = client.list_containers().into_stream();
            stream
                .try_for_each_concurrent(None, |list_response| async {
                    match futures::future::join_all(list_response.containers.into_iter().map(
                        |container| async {
                            client.container_client(container.name).delete().await
                        },
                    ))
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>, azure_storage::Error>>()
                    {
                        Ok(_) => Ok(()),
                        Err(err) => Err(err.context("failed to delete container")),
                    }
                })
                .await
                .map_err(|e| anyhow::anyhow!(e))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn container_exists(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<bool, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(name)
                .exists()
                .await
                .context("failed to check container existence")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn create_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(name)
                .create()
                .await
                .context("failed to create container")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(name)
                .delete()
                .await
                .context("failed to delete container")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_container_info(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<ContainerMetadata, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let properties = client
                .container_client(name)
                .get_properties()
                .await
                .context("failed to get container properties")?;

            let created_at = properties
                .date
                .unix_timestamp()
                .try_into()
                .context("failed to convert created_at date to u64")?;

            // NOTE: The `created_at` format is currently undefined
            // https://github.com/WebAssembly/wasi-blobstore/issues/7
            anyhow::Ok(ContainerMetadata { created_at })
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn list_container_objects(
        &self,
        cx: Option<Context>,
        name: String,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> anyhow::Result<Result<Pin<Box<dyn Stream<Item = Vec<String>> + Send>>, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let mut names = client.container_client(name).list_blobs().into_stream();
            let (tx, rx) = mpsc::channel(16);
            spawn(async move {
                let mut offset = offset.unwrap_or_default().try_into().unwrap_or(usize::MAX);
                let mut limit = limit
                    .and_then(|limit| limit.try_into().ok())
                    .unwrap_or(usize::MAX);
                while let Some(res) = names.next().await {
                    match res {
                        Ok(res) => {
                            let mut chunk = vec![];
                            for name in res.blobs.blobs().map(|Blob { name, .. }| name) {
                                if limit == 0 {
                                    return;
                                }
                                if offset > 0 {
                                    offset -= 1;
                                    continue;
                                }
                                chunk.push(name.clone());
                                limit -= 1;
                            }
                            if tx.send(chunk).await.is_err() {
                                warn!("stream receiver closed");
                                return;
                            }
                        }
                        Err(err) => {
                            error!(?err, "failed to receive response");
                            return;
                        }
                    }
                }
            });
            anyhow::Ok(Box::pin(ReceiverStream::new(rx)) as Pin<Box<dyn Stream<Item = _> + Send>>)
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn copy_object(
        &self,
        cx: Option<Context>,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let copy_source = client
                .container_client(src.container)
                .blob_client(src.object)
                .url()
                .context("failed to get source object for copy")?;

            client
                .container_client(dest.container)
                .blob_client(dest.object)
                .copy(copy_source)
                .await
                .map(|_| ())
                .context("failed to copy source object")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_object(
        &self,
        cx: Option<Context>,
        id: ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(id.container)
                .blob_client(id.object)
                .delete()
                .await
                .map(|_| ())
                .context("failed to delete object")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_objects(
        &self,
        cx: Option<Context>,
        container: String,
        objects: Vec<String>,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let deletes = objects.iter().map(|object| async {
                client
                    .container_client(container.clone())
                    .blob_client(object.clone())
                    .delete()
                    .await
            });
            futures::future::join_all(deletes)
                .await
                .into_iter()
                .collect::<Result<Vec<_>, azure_storage::Error>>()
                .map(|_| ())
                .context("failed to delete objects")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_container_data(
        &self,
        cx: Option<Context>,
        id: ObjectId,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Result<Pin<Box<dyn Stream<Item = Bytes> + Send>>, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let mut stream = client
                .container_client(id.container)
                .blob_client(id.object)
                .get()
                .range(start..end)
                .into_stream();

            let (tx, rx) = mpsc::channel(16);
            spawn(async move {
                while let Some(res) = stream.next().await {
                    match res {
                        Ok(res) => match res.data.collect().await {
                            Ok(buf) => {
                                if tx.send(buf).await.is_err() {
                                    warn!("stream receiver closed");
                                    return;
                                }
                            }
                            Err(err) => {
                                error!(?err, "failed to receive bytes");
                                return;
                            }
                        },
                        Err(err) => {
                            error!(?err, "failed to receive blob");
                            return;
                        }
                    }
                }
            });
            anyhow::Ok(Box::pin(ReceiverStream::new(rx)) as Pin<Box<dyn Stream<Item = _> + Send>>)
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_object_info(
        &self,
        cx: Option<Context>,
        id: ObjectId,
    ) -> anyhow::Result<Result<ObjectMetadata, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let info = client
                .container_client(id.container)
                .blob_client(id.object)
                .get_properties()
                .await
                .map_err(|e| anyhow::anyhow!(e))?;

            // NOTE: The `created_at` format is currently undefined
            // https://github.com/WebAssembly/wasi-blobstore/issues/7
            let created_at = info
                .date
                .unix_timestamp()
                .try_into()
                .context("failed to convert created_at date to u64")?;
            anyhow::Ok(ObjectMetadata {
                created_at,
                size: info.blob.properties.content_length,
            })
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn has_object(
        &self,
        cx: Option<Context>,
        id: ObjectId,
    ) -> anyhow::Result<Result<bool, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(id.container)
                .blob_client(id.object)
                .exists()
                .await
                .map_err(|e| anyhow::anyhow!(e))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn move_object(
        &self,
        cx: Option<Context>,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let source_client = client
                .container_client(src.container)
                .blob_client(src.object);

            // Copy and then delete the source object
            let copy_source = source_client
                .url()
                .context("failed to get source object for copy")?;

            client
                .container_client(dest.container)
                .blob_client(dest.object)
                .copy(copy_source)
                .await
                .map(|_| ())
                .context("failed to copy source object to move")?;

            source_client
                .delete()
                .await
                .map(|_| ())
                .context("failed to delete source object")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self, data))]
    async fn write_container_data(
        &self,
        cx: Option<Context>,
        id: ObjectId,
        data: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            // TODO: Consider streaming
            let data: BytesMut = data.collect().await;
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(id.container)
                .blob_client(id.object)
                .put_block_blob(data)
                .await
                .map(|_| ())
                .context("failed to write container data")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }
}
