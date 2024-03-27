use core::future::Future;
use core::pin::pin;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use tokio::{select, spawn};
use tracing::{debug, error, instrument, warn};
use wrpc_interface_blobstore::ObjectId;
use wrpc_transport::{AcceptedInvocation, Transmitter};

use crate::{get_connection, Context};

use super::WrpcContextClient;

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

#[instrument(level = "debug", skip_all)]
pub async fn serve_blobstore(
    provider: impl Blobstore + Clone + 'static,
    commands: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let connection = get_connection();
    let wrpc = WrpcContextClient(connection.get_wrpc_client(connection.provider_key()));
    let mut commands = pin!(commands);
    'outer: loop {
        use wrpc_interface_blobstore::Blobstore as _;
        let clear_container_invocations = wrpc
            .serve_clear_container()
            .await
            .context("failed to serve `wrpc:blobstore/blobstore.clear-container` invocations")?;
        let mut clear_container_invocations = pin!(clear_container_invocations);

        let container_exists_invocations = wrpc
            .serve_container_exists()
            .await
            .context("failed to serve `wrpc:blobstore/blobstore.container-exists` invocations")?;
        let mut container_exists_invocations = pin!(container_exists_invocations);

        let create_container_invocations = wrpc
            .serve_create_container()
            .await
            .context("failed to serve `wrpc:blobstore/blobstore.create-container` invocations")?;
        let mut create_container_invocations = pin!(create_container_invocations);

        let delete_container_invocations = wrpc
            .serve_delete_container()
            .await
            .context("failed to serve `wrpc:blobstore/blobstore.delete-container` invocations")?;
        let mut delete_container_invocations = pin!(delete_container_invocations);

        let get_container_info_invocations = wrpc
            .serve_get_container_info()
            .await
            .context("failed to serve `wrpc:blobstore/blobstore.get-container-info` invocations")?;
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

        let get_container_data_invocations = wrpc
            .serve_get_container_data()
            .await
            .context("failed to serve `wrpc:blobstore/blobstore.get-container-data` invocations")?;
        let mut get_container_data_invocations = pin!(get_container_data_invocations);

        let get_object_info_invocations = wrpc
            .serve_get_object_info()
            .await
            .context("failed to serve `wrpc:blobstore/blobstore.get-object-info` invocations")?;
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

        let write_container_data_invocations = wrpc.serve_write_container_data().await.context(
            "failed to serve `wrpc:blobstore/blobstore.write-container-data` invocations",
        )?;
        let mut write_container_data_invocations = pin!(write_container_data_invocations);

        loop {
            select! {
                invocation = clear_container_invocations.next() => {
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
                invocation = container_exists_invocations.next() => {
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
                invocation = create_container_invocations.next() => {
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
                invocation = delete_container_invocations.next() => {
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
                invocation = get_container_info_invocations.next() => {
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
                invocation = list_container_objects_invocations.next() => {
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
                invocation = copy_object_invocations.next() => {
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
                invocation = delete_object_invocations.next() => {
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
                invocation = delete_objects_invocations.next() => {
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
                invocation = get_container_data_invocations.next() => {
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
                invocation = get_object_info_invocations.next() => {
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
                invocation = has_object_invocations.next() => {
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
                invocation = move_object_invocations.next() => {
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
                invocation = write_container_data_invocations.next() => {
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
                _ = &mut commands => {
                    debug!("shutdown command received");
                    return Ok(())
                }
            }
        }
    }
}
