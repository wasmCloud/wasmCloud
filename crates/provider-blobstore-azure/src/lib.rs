use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use azure_storage_blobs::prelude::*;
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt, TryStreamExt as _};
use tokio::sync::RwLock;
use tracing::{error, instrument};
use wasmcloud_provider_sdk::interfaces::blobstore::Blobstore;
use wasmcloud_provider_sdk::{Context, LinkConfig, Provider};
use wrpc_transport_legacy::{AcceptedInvocation, Transmitter};

use config::StorageConfig;

mod config;

/// Blobstore Azblob provider
///
/// This struct will be the target of generated implementations (via wit-provider-bindgen)
/// for the blobstore provider WIT contract
#[derive(Default, Clone)]
pub struct BlobstoreAzblobProvider {
    /// Per-config storage for Azure connection clients
    config: Arc<RwLock<HashMap<String, BlobServiceClient>>>,
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

impl Blobstore for BlobstoreAzblobProvider {
    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_clear_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
                        .await
                        .context("failed to retrieve azure blobstore client")?;

                    let stream = client.list_containers().into_stream();
                    stream
                        .try_for_each_concurrent(None, |list_response| async {
                            match futures::future::join_all(
                                list_response.containers.into_iter().map(|container| async {
                                    client.container_client(container.name).delete().await
                                }),
                            )
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
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_container_exists<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
                        .await
                        .context("failed to retrieve azure blobstore client")?;

                    client
                        .container_client(container)
                        .exists()
                        .await
                        .context("failed to check container existence")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_create_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
                        .await
                        .context("failed to retrieve azure blobstore client")?;

                    client
                        .container_client(container)
                        .create()
                        .await
                        .context("failed to create container")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_delete_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
                        .await
                        .context("failed to retrieve azure blobstore client")?;

                    client
                        .container_client(container)
                        .delete()
                        .await
                        .context("failed to delete container")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_get_container_info<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
                        .await
                        .context("failed to retrieve azure blobstore client")?;

                    let properties = client
                        .container_client(container)
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
                    anyhow::Ok(wrpc_interface_blobstore::ContainerMetadata { created_at })
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[allow(clippy::type_complexity)]
    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_list_container_objects<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (container, limit, offset),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, (String, Option<u64>, Option<u64>), Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
                        .await
                        .context("failed to retrieve azure blobstore client")?;

                    let stream = client
                        .container_client(container)
                        .list_blobs()
                        .into_stream();
                    let container_names = stream
                        .map(|res| {
                            res.map(|list_response| {
                                list_response
                                    .blobs
                                    .blobs()
                                    .map(|blob| {
                                        Some(wrpc_transport_legacy::Value::String(
                                            blob.name.clone(),
                                        ))
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .map_err(|e| anyhow::anyhow!(e))
                        })
                        .skip(offset.unwrap_or_default().try_into().unwrap_or(usize::MAX))
                        .take(
                            limit
                                .and_then(|limit| limit.try_into().ok())
                                .unwrap_or(usize::MAX),
                        );

                    anyhow::Ok(wrpc_transport_legacy::Value::Stream(Box::pin(
                        container_names,
                    )))
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_copy_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (src, dest),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (
                wrpc_interface_blobstore::ObjectId,
                wrpc_interface_blobstore::ObjectId,
            ),
            Tx,
        >,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
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
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_delete_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: id,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, wrpc_interface_blobstore::ObjectId, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
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
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_delete_objects<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (container, objects),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, (String, Vec<String>), Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
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
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_get_container_data<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (id, start, end),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (wrpc_interface_blobstore::ObjectId, u64, u64),
            Tx,
        >,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
                        .await
                        .context("failed to retrieve azure blobstore client")?;

                    let stream = client
                        .container_client(id.container)
                        .blob_client(id.object)
                        .get()
                        .range(start..end)
                        .into_stream();

                    let data = stream
                        .map_err(|e| anyhow::anyhow!(e))
                        .and_then(|res| async {
                            Ok(vec![Some(wrpc_transport_legacy::Value::List(
                                res.data
                                    .collect()
                                    .await
                                    .map(|bytes| {
                                        bytes
                                            .into_iter()
                                            .map(wrpc_transport_legacy::Value::U8)
                                            .collect()
                                    })
                                    .map_err(|e| anyhow::anyhow!(e))?,
                            ))])
                        });

                    anyhow::Ok(wrpc_transport_legacy::Value::Stream(Box::pin(data)))
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_get_object_info<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: id,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, wrpc_interface_blobstore::ObjectId, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
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
                    anyhow::Ok(wrpc_interface_blobstore::ObjectMetadata {
                        created_at: info
                            .date
                            .unix_timestamp()
                            .try_into()
                            .context("failed to convert created_at date to u64")?,
                        size: info.blob.properties.content_length,
                    })
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_has_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: id,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, wrpc_interface_blobstore::ObjectId, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
                        .await
                        .context("failed to retrieve azure blobstore client")?;

                    client
                        .container_client(id.container)
                        .blob_client(id.object)
                        .exists()
                        .await
                        .map_err(|e| anyhow::anyhow!(e))
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_move_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (src, dest),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (
                wrpc_interface_blobstore::ObjectId,
                wrpc_interface_blobstore::ObjectId,
            ),
            Tx,
        >,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
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
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(
        level = "trace",
        skip(self, result_subject, error_subject, transmitter, data)
    )]
    async fn serve_write_container_data<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (id, data),
            result_subject,
            error_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (
                wrpc_interface_blobstore::ObjectId,
                impl Stream<Item = anyhow::Result<Bytes>> + Send,
            ),
            Tx,
        >,
    ) {
        // TODO: Consider streaming
        let data: BytesMut = match data.try_collect().await {
            Ok(data) => data,
            Err(err) => {
                error!(?err, "failed to receive value");
                if let Err(err) = transmitter
                    .transmit_static(error_subject, err.to_string())
                    .await
                {
                    error!(?err, "failed to transmit error")
                }
                return;
            }
        };
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let client = self
                        .get_config(context.as_ref())
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
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }
}
