use core::future::Future;
use core::pin::pin;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use tokio::{select, spawn};
use tracing::{debug, error, instrument, warn};
use wrpc_transport::{AcceptedInvocation, Transmitter};

use crate::{get_connection, Context};

use super::WrpcContextClient;

pub trait Eventual: Send {
    fn serve_delete<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (String, String), Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_exists<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (String, String), Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_get<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (String, String), Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_set<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<
            Option<Context>,
            (
                String,
                String,
                impl Stream<Item = anyhow::Result<Bytes>> + Send,
            ),
            Tx,
        >,
    ) -> impl Future<Output = ()> + Send;
}

pub trait Atomic: Send {
    fn serve_compare_and_swap<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (String, String, u64, u64), Tx>,
    ) -> impl Future<Output = ()> + Send;

    fn serve_increment<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<Option<Context>, (String, String, u64), Tx>,
    ) -> impl Future<Output = ()> + Send;
}

// TODO: This should probably be split into two
#[instrument(level = "trace", skip_all)]
pub async fn serve_keyvalue(
    provider: impl Atomic + Eventual + Clone + 'static,
    commands: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let connection = get_connection();
    let wrpc = WrpcContextClient(connection.get_wrpc_client(connection.provider_key()));
    let mut commands = pin!(commands);
    'outer: loop {
        use wrpc_interface_keyvalue::{Atomic as _, Eventual as _};
        let delete_invocations = wrpc
            .serve_delete()
            .await
            .context("failed to serve `wrpc:keyvalue/eventual.delete` invocations")?;
        let mut delete_invocations = pin!(delete_invocations);

        let exists_invocations = wrpc
            .serve_exists()
            .await
            .context("failed to serve `wrpc:keyvalue/eventual.exists` invocations")?;
        let mut exists_invocations = pin!(exists_invocations);

        let get_invocations = wrpc
            .serve_get()
            .await
            .context("failed to serve `wrpc:keyvalue/eventual.get` invocations")?;
        let mut get_invocations = pin!(get_invocations);

        let set_invocations = wrpc
            .serve_set()
            .await
            .context("failed to serve `wrpc:keyvalue/eventual.set` invocations")?;
        let mut set_invocations = pin!(set_invocations);

        let compare_and_swap_invocations = wrpc
            .serve_compare_and_swap()
            .await
            .context("failed to serve `wrpc:keyvalue/atomic.compare-and-swap` invocations")?;
        let mut compare_and_swap_invocations = pin!(compare_and_swap_invocations);

        let increment_invocations = wrpc
            .serve_increment()
            .await
            .context("failed to serve `wrpc:keyvalue/atomic.increment` invocations")?;
        let mut increment_invocations = pin!(increment_invocations);
        loop {
            select! {
                invocation = delete_invocations.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
                            spawn(async move { provider.serve_delete(invocation).await });
                        },
                        Some(Err(err)) => {
                            error!(?err, "failed to accept `wrpc:keyvalue/eventual.delete` invocation")
                        },
                        None => {
                            warn!("`wrpc:keyvalue/eventual.delete` stream unexpectedly finished, resubscribe");
                            continue 'outer
                        }
                    }
                }
                invocation = exists_invocations.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
                            spawn(async move { provider.serve_exists(invocation).await });
                        },
                        Some(Err(err)) => {
                            error!(?err, "failed to accept `wrpc:keyvalue/eventual.exists` invocation")
                        },
                        None => {
                            warn!("`wrpc:keyvalue/eventual.exists` stream unexpectedly finished, resubscribe");
                            continue 'outer
                        }
                    }
                }
                invocation = get_invocations.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
                            spawn(async move { provider.serve_get(invocation).await });
                        },
                        Some(Err(err)) => {
                            error!(?err, "failed to accept `wrpc:keyvalue/eventual.get` invocation")
                        },
                        None => {
                            warn!("`wrpc:keyvalue/eventual.get` stream unexpectedly finished, resubscribe");
                            continue 'outer
                        }
                    }
                }
                invocation = set_invocations.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
                            spawn(async move { provider.serve_set(invocation).await });
                        },
                        Some(Err(err)) => {
                            error!(?err, "failed to accept `wrpc:keyvalue/eventual.set` invocation")
                        },
                        None => {
                            warn!("`wrpc:keyvalue/eventual.set` stream unexpectedly finished, resubscribe");
                            continue 'outer
                        }
                    }
                }
                invocation = compare_and_swap_invocations.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
                            spawn(async move { provider.serve_compare_and_swap(invocation).await });
                        },
                        Some(Err(err)) => {
                            error!(?err, "failed to accept `wrpc:keyvalue/atomic.compare-and-swamp` invocation")
                        },
                        None => {
                            warn!("`wrpc:keyvalue/atomic.compare-and-swamp` stream unexpectedly finished, resubscribe");
                            continue 'outer
                        }
                    }
                }
                invocation = increment_invocations.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            let provider = provider.clone();
                            spawn(async move { provider.serve_increment(invocation).await });
                        },
                        Some(Err(err)) => {
                            error!(?err, "failed to accept `wrpc:keyvalue/atomic.increment` invocation")
                        },
                        None => {
                            warn!("`wrpc:keyvalue/atomic.increment` stream unexpectedly finished, resubscribe");
                            continue 'outer
                        }
                    }
                }
                _ = &mut commands => {
                    debug!("shutdown command received");
                    return Ok(())
                }
            }
        }
    }
}
