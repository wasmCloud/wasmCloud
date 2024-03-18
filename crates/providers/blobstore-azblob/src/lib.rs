use core::future::Future;
use core::pin::pin;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use async_nats::HeaderMap;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::{Stream, TryStreamExt as _};
use tokio::sync::RwLock;
use tokio::{select, spawn};
use tracing::{debug, error, instrument, warn};
use wasmcloud_provider_sdk::provider::invocation_context;
use wasmcloud_provider_sdk::{
    get_connection, Context, LinkConfig, ProviderHandler, ProviderOperationResult,
};
use wrpc_transport::{AcceptedInvocation, Transmitter};

use anyhow::Result;
use azure_storage_blobs::prelude::*;
use config::StorageConfig;
use futures::StreamExt;

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
#[async_trait]
impl ProviderHandler for BlobstoreAzblobProvider {
    #[instrument(level = "info", skip_all)]
    async fn receive_link_config_as_target(
        &self,
        link_config: impl LinkConfig,
    ) -> ProviderOperationResult<()> {
        let config = link_config.get_config();
        let config = match StorageConfig::from_values(config) {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, source_id = %link_config.get_source_id(), "failed to read storage config");
                return Err(e.into());
            }
        };
        let link =
            BlobServiceClient::builder(config.storage_account.clone(), config.configure_az())
                .blob_service_client();

        let mut update_map = self.config.write().await;
        update_map.insert(link_config.get_source_id().to_string(), link);

        Ok(())
    }

    async fn delete_link(&self, source_id: &str) -> ProviderOperationResult<()> {
        self.config.write().await.remove(source_id);
        Ok(())
    }

    async fn shutdown(&self) -> ProviderOperationResult<()> {
        self.config.write().await.drain();
        Ok(())
    }
}

impl BlobstoreAzblobProvider {
    async fn get_config(&self, headers: Option<&HeaderMap>) -> anyhow::Result<BlobServiceClient> {
        if let Some(ref source_id) = headers
            .map(invocation_context)
            .and_then(|Context { actor, .. }| actor)
        {
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

    #[instrument(level = "trace", skip_all)]
    pub async fn serve(&self, commands: impl Future<Output = ()>) -> anyhow::Result<()> {
        let connection = get_connection();
        let wrpc = connection.get_wrpc_client(connection.provider_key());
        let mut commands = pin!(commands);
        'outer: loop {
            use wrpc_interface_blobstore::Blobstore as _;
            let clear_container_invocations = wrpc.serve_clear_container().await.context(
                "failed to serve `wrpc:blobstore/blobstore.clear-container` invocations",
            )?;
            let mut clear_container_invocations = pin!(clear_container_invocations);

            let container_exists_invocations = wrpc.serve_container_exists().await.context(
                "failed to serve `wrpc:blobstore/blobstore.container-exists` invocations",
            )?;
            let mut container_exists_invocations = pin!(container_exists_invocations);

            let create_container_invocations = wrpc.serve_create_container().await.context(
                "failed to serve `wrpc:blobstore/blobstore.create-container` invocations",
            )?;
            let mut create_container_invocations = pin!(create_container_invocations);

            let delete_container_invocations = wrpc.serve_delete_container().await.context(
                "failed to serve `wrpc:blobstore/blobstore.delete-container` invocations",
            )?;
            let mut delete_container_invocations = pin!(delete_container_invocations);

            let get_container_info_invocations = wrpc.serve_get_container_info().await.context(
                "failed to serve `wrpc:blobstore/blobstore.get-container-info` invocations",
            )?;
            let mut get_container_info_invocations = pin!(get_container_info_invocations);

            let list_container_objects_invocations =
                wrpc.serve_list_container_objects().await.context(
                    "failed to serve `wrpc:blobstore/blobstore.list-container-objects` invocations",
                )?;
            let mut list_container_objects_invocations = pin!(list_container_objects_invocations);

            let copy_object_invocations = wrpc
                .serve_copy_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.copy-object` invocations")?;
            let mut copy_object_invocations = pin!(copy_object_invocations);

            let delete_object_invocations = wrpc
                .serve_delete_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.delete-object` invocations")?;
            let mut delete_object_invocations = pin!(delete_object_invocations);

            let delete_objects_invocations = wrpc
                .serve_delete_objects()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.delete-objects` invocations")?;
            let mut delete_objects_invocations = pin!(delete_objects_invocations);

            let get_container_data_invocations = wrpc.serve_get_container_data().await.context(
                "failed to serve `wrpc:blobstore/blobstore.get-container-data` invocations",
            )?;
            let mut get_container_data_invocations = pin!(get_container_data_invocations);

            let get_object_info_invocations = wrpc.serve_get_object_info().await.context(
                "failed to serve `wrpc:blobstore/blobstore.get-object-info` invocations",
            )?;
            let mut get_object_info_invocations = pin!(get_object_info_invocations);

            let has_object_invocations = wrpc
                .serve_has_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.has-object` invocations")?;
            let mut has_object_invocations = pin!(has_object_invocations);

            let move_object_invocations = wrpc
                .serve_move_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.move-object` invocations")?;
            let mut move_object_invocations = pin!(move_object_invocations);

            let write_container_data_invocations =
                wrpc.serve_write_container_data().await.context(
                    "failed to serve `wrpc:blobstore/blobstore.write-container-data` invocations",
                )?;
            let mut write_container_data_invocations = pin!(write_container_data_invocations);

            loop {
                select! {
                    invocation = clear_container_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_clear_container(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.clear-container` invocation")
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.clear-container` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            }
                        }
                    },
                    invocation = container_exists_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_container_exists(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.container-exists` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.container-exists` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = create_container_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_create_container(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.container-exists` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.container-exists` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = delete_container_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_delete_container(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.delete-container` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.delete-container` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = get_container_info_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_get_container_info(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.get-container-info` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.get-container-info` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = list_container_objects_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_list_container_objects(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.list-container-objects` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.list-container-objects` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = copy_object_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_copy_object(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.copy-object` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.copy-object` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = delete_object_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_delete_object(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.delete-object` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.delete-object` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = delete_objects_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_delete_objects(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.delete-objects` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.delete-objects` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = get_container_data_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_get_container_data(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.get-container-data` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.get-container-data` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = get_object_info_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_get_object_info(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.get-object-info` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.get-object-info` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = has_object_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_has_object(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.has-object` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.has-object` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = move_object_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_move_object(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.move-object` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.move-object` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = write_container_data_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_write_container_data(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.write-container-data` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.write-container-data` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    _ = &mut commands => {
                        debug!("shutdown command received");
                        return Ok(())
                    }
                }
            }
        }
    }
}

impl BlobstoreAzblobProvider {
    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_clear_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
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
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
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
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
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
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
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
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
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
        }: AcceptedInvocation<Option<HeaderMap>, (String, Option<u64>, Option<u64>), Tx>,
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
                    let container_names = stream
                        .map(|res| {
                            res.map(|list_response| {
                                list_response
                                    .containers
                                    .iter()
                                    .map(|container| {
                                        Some(wrpc_transport::Value::String(container.name.clone()))
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .map_err(|e| anyhow::anyhow!(e))
                        })
                        .skip(offset.unwrap_or_default().try_into().unwrap_or(usize::MAX))
                        .take(limit.unwrap_or_default().try_into().unwrap_or(usize::MAX));

                    anyhow::Ok(wrpc_transport::Value::Stream(Box::pin(container_names)))
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
            Option<HeaderMap>,
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
        }: AcceptedInvocation<Option<HeaderMap>, wrpc_interface_blobstore::ObjectId, Tx>,
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
        }: AcceptedInvocation<Option<HeaderMap>, (String, Vec<String>), Tx>,
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
            Option<HeaderMap>,
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
                            Ok(vec![Some(wrpc_transport::Value::List(
                                res.data
                                    .collect()
                                    .await
                                    .map(|bytes| {
                                        bytes.into_iter().map(wrpc_transport::Value::U8).collect()
                                    })
                                    .map_err(|e| anyhow::anyhow!(e))?,
                            ))])
                        });

                    anyhow::Ok(wrpc_transport::Value::Stream(Box::pin(data)))
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
        }: AcceptedInvocation<Option<HeaderMap>, wrpc_interface_blobstore::ObjectId, Tx>,
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
        }: AcceptedInvocation<Option<HeaderMap>, wrpc_interface_blobstore::ObjectId, Tx>,
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
            Option<HeaderMap>,
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
            Option<HeaderMap>,
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
