use core::future::Future;
use core::pin::pin;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use tokio::{select, spawn};
use tracing::{debug, error, instrument, warn};
use wrpc_interface_blobstore::{BlobstoreInvocations, ObjectId};
use wrpc_transport::{AcceptedInvocation, Transmitter};

use crate::{get_connection, run_provider_handler, Context, ProviderHandler};

use super::WrpcContextClient;

/// `wrpc:blobstore/blobstore` provider
pub trait Blobstore: Send {
    fn serve_clear_container<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, String, Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_container_exists<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, String, Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_create_container<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, String, Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_delete_container<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, String, Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_get_container_info<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, String, Tx>,
    ) -> impl Future<Output = ()> + Send;

    #[allow(clippy::type_complexity)]
    fn serve_list_container_objects<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (String, Option<u64>, Option<u64>), Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_copy_object<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (ObjectId, ObjectId), Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_delete_object<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, ObjectId, Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_delete_objects<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (String, Vec<String>), Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_get_container_data<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (ObjectId, u64, u64), Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_get_object_info<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, ObjectId, Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_has_object<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, ObjectId, Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_move_object<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (ObjectId, ObjectId), Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_write_container_data<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<
            Option<Context>,
            (ObjectId, impl Stream<Item = anyhow::Result<Bytes>> + Send),
            Tx,
        >,
    ) -> impl Future<Output = ()> + Send;
}

/// Serve `wrpc:blobstore/blobstore` provider until shutdown is received
#[instrument(level = "debug", skip_all)]
pub async fn serve_blobstore(
    provider: impl Blobstore + Clone + 'static,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let connection = get_connection();
    let wrpc = WrpcContextClient(connection.get_wrpc_client(connection.provider_key()));
    let mut shutdown = pin!(shutdown);
    'outer: loop {
        let BlobstoreInvocations {
            mut clear_container,
            mut container_exists,
            mut create_container,
            mut delete_container,
            mut get_container_info,
            mut list_container_objects,
            mut copy_object,
            mut delete_object,
            mut delete_objects,
            mut get_container_data,
            mut get_object_info,
            mut has_object,
            mut move_object,
            mut write_container_data,
        } = wrpc_interface_blobstore::serve_blobstore(&wrpc).await?;
        loop {
            select! {
                invocation = clear_container.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = container_exists.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = create_container.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = delete_container.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = get_container_info.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = list_container_objects.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = copy_object.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = delete_object.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = delete_objects.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = get_container_data.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = get_object_info.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = has_object.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = move_object.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                invocation = write_container_data.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
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
                _ = &mut shutdown => {
                    debug!("shutdown command received");
                    return Ok(())
                }
            }
        }
    }
}

/// Run `wrpc:blobstore/blobstore` provider
#[instrument(level = "trace", skip_all)]
pub async fn run_blobstore(
    provider: impl ProviderHandler + Blobstore + Clone + 'static,
    name: &str,
) -> anyhow::Result<()> {
    let shutdown = run_provider_handler(provider.clone(), name)
        .await
        .context("failed to run provider")?;
    serve_blobstore(provider, shutdown).await
}
