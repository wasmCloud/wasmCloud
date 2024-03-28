use core::future::Future;
use core::pin::pin;

use anyhow::Context as _;
use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use tokio::{select, spawn, try_join};
use tracing::{debug, error, instrument, warn};
use wrpc_interface_keyvalue::{AtomicInvocations, EventualInvocations};
use wrpc_transport::{AcceptedInvocation, Transmitter};

use crate::{get_connection, run_provider_handler, Context, ProviderHandler};

use super::WrpcContextClient;

/// `wrpc:keyvalue/atomic` provider
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

/// `wrpc:keyvalue/eventual` provider
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

// TODO: remove `serve` duplication

/// Serve `wrpc:keyvalue/atomic` provider until shutdown is received
#[instrument(level = "trace", skip_all)]
pub async fn serve_atomic(
    provider: impl Atomic + Clone + 'static,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let connection = get_connection();
    let wrpc = WrpcContextClient(connection.get_wrpc_client(connection.provider_key()));
    let mut shutdown = pin!(shutdown);
    'outer: loop {
        let AtomicInvocations {
            mut compare_and_swap,
            mut increment,
        } = wrpc_interface_keyvalue::serve_atomic(&wrpc).await?;
        loop {
            select! {
                invocation = compare_and_swap.next() => {
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
                invocation = increment.next() => {
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
                _ = &mut shutdown => {
                    debug!("shutdown command received");
                    return Ok(())
                }
            }
        }
    }
}

/// Serve `wrpc:keyvalue/eventual` provider until shutdown is received
#[instrument(level = "trace", skip_all)]
pub async fn serve_eventual(
    provider: impl Eventual + Clone + 'static,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let connection = get_connection();
    let wrpc = WrpcContextClient(connection.get_wrpc_client(connection.provider_key()));
    let mut shutdown = pin!(shutdown);
    'outer: loop {
        let EventualInvocations {
            mut delete,
            mut exists,
            mut get,
            mut set,
        } = wrpc_interface_keyvalue::serve_eventual(&wrpc).await?;
        loop {
            select! {
                invocation = delete.next() => {
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
                invocation = exists.next() => {
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
                invocation = get.next() => {
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
                invocation = set.next() => {
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
                _ = &mut shutdown => {
                    debug!("shutdown command received");
                    return Ok(())
                }
            }
        }
    }
}

/// Serve all supported `wrpc:keyvalue` interface provider until shutdown is received
#[instrument(level = "trace", skip_all)]
pub async fn serve(
    provider: impl Atomic + Eventual + Clone + 'static,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let connection = get_connection();
    let wrpc = WrpcContextClient(connection.get_wrpc_client(connection.provider_key()));
    let mut shutdown = pin!(shutdown);
    'outer: loop {
        let (
            AtomicInvocations {
                mut compare_and_swap,
                mut increment,
            },
            EventualInvocations {
                mut delete,
                mut exists,
                mut get,
                mut set,
            },
        ) = try_join!(
            wrpc_interface_keyvalue::serve_atomic(&wrpc),
            wrpc_interface_keyvalue::serve_eventual(&wrpc),
        )?;
        loop {
            select! {
                invocation = delete.next() => {
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
                invocation = exists.next() => {
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
                invocation = get.next() => {
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
                invocation = set.next() => {
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
                invocation = compare_and_swap.next() => {
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
                invocation = increment.next() => {
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
                _ = &mut shutdown => {
                    debug!("shutdown command received");
                    return Ok(())
                }
            }
        }
    }
}

/// Run `wrpc:keyvalue/atomic` provider
#[instrument(level = "trace", skip_all)]
pub async fn run_atomic(
    provider: impl ProviderHandler + Atomic + Clone + 'static,
    name: &str,
) -> anyhow::Result<()> {
    let shutdown = run_provider_handler(provider.clone(), name)
        .await
        .context("failed to run provider")?;
    serve_atomic(provider, shutdown).await
}

/// Run `wrpc:keyvalue/eventual` provider
#[instrument(level = "trace", skip_all)]
pub async fn run_eventual(
    provider: impl ProviderHandler + Eventual + Clone + 'static,
    name: &str,
) -> anyhow::Result<()> {
    let shutdown = run_provider_handler(provider.clone(), name)
        .await
        .context("failed to run provider")?;
    serve_eventual(provider, shutdown).await
}

/// Run all supported `wrpc:keyvalue` interface provider
#[instrument(level = "trace", skip_all)]
pub async fn run(
    provider: impl ProviderHandler + Atomic + Eventual + Clone + 'static,
    name: &str,
) -> anyhow::Result<()> {
    let shutdown = run_provider_handler(provider.clone(), name)
        .await
        .context("failed to run provider")?;
    serve(provider, shutdown).await
}
