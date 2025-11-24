use core::net::SocketAddr;
use core::str::FromStr as _;

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use futures::{stream, Stream, StreamExt as _};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinSet;
use tracing::{error, info, instrument, trace, warn};
use wasmcloud_core::http::default_listen_address;
use wasmcloud_provider_sdk::provider::InvocationStreams;
use wasmcloud_provider_sdk::ProviderConnection;

use crate::bindings;
use crate::wasmbus::providers::{check_health, watch_config};

pub(crate) mod address;
pub(crate) mod host;
pub(crate) mod path;

/// Helper enum to allow for code reuse between different routing modes
enum HttpServerProvider {
    Address(address::Provider),
    Path(path::Provider),
    Host(host::Provider),
}

impl crate::wasmbus::Host {
    /// Initializes and starts the internal HTTP server provider.
    ///
    /// The provider routing mode and default address are determined from config.
    /// Links trigger actual HTTP listener startup via update_interface_import_config.
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn start_http_server_provider(
        self: Arc<Self>,
        provider_id: &str,
        config_names: Vec<String>,
        annotations: BTreeMap<String, String>,
    ) -> anyhow::Result<JoinSet<()>> {
        info!(
            "Starting internal HTTP server provider with ID: {}",
            provider_id
        );

        let host_id = self.host_key.public_key();

        // Resolve config to get routing_mode and default_address
        // These determine the provider variant at startup
        let (config_bundle, _secrets) = self
            .fetch_config_and_secrets(
                &config_names,
                None, // No claims for builtin providers
                annotations.get("wasmcloud.dev/appspec"),
            )
            .await?;
        let host_config = config_bundle.get_config().await;

        let default_address = host_config
            .get("default_address")
            .map(|s| SocketAddr::from_str(s))
            .transpose()
            .context("failed to parse default_address")?
            .unwrap_or_else(default_listen_address);

        // Create broadcast channel for shutdown signaling before constructing provider variants
        let (quit_tx, quit_rx) = broadcast::channel(1);
        let config_shutdown_rx = quit_tx.subscribe();

        let provider = match host_config.get("routing_mode").map(String::as_str) {
            // Run provider in address mode by default
            Some("address") | None => HttpServerProvider::Address(address::Provider {
                address: default_address,
                components: Arc::clone(&self.components),
                links: Arc::default(),
                host_id: Arc::from(host_id.as_str()),
                lattice_id: Arc::clone(&self.host_config.lattice),
                quit_tx: quit_tx.clone(),
            }),
            // Run provider in path mode
            Some("path") => HttpServerProvider::Path(
                path::Provider::new(
                    default_address,
                    Arc::clone(&self.components),
                    Arc::from(host_id.as_str()),
                    Arc::clone(&self.host_config.lattice),
                    quit_tx.clone(),
                )
                .await?,
            ),
            Some("host") => HttpServerProvider::Host(
                host::Provider::new(
                    default_address,
                    Arc::clone(&self.components),
                    Arc::from(host_id.as_str()),
                    Arc::clone(&self.host_config.lattice),
                    host_config.get("header").cloned(),
                    quit_tx.clone(),
                )
                .await?,
            ),
            Some(other) => bail!("unknown routing_mode: {other}"),
        };

        let conn = ProviderConnection::new(
            Arc::clone(&self.rpc_nats),
            Arc::from(provider_id),
            Arc::clone(&self.host_config.lattice),
            host_id.to_string(),
        )
        .context("failed to establish provider connection")?;

        let extension_wrpc_client = conn
            .get_wrpc_extension_serve_client_custom(None)
            .await
            .context("failed to create extension wRPC client")?;

        let mut tasks = JoinSet::new();
        let provider_id_for_task = provider_id.to_string();

        let extension_invocations = match provider {
            HttpServerProvider::Address(provider) => {
                bindings::serve(&extension_wrpc_client, provider).await
            }
            HttpServerProvider::Path(provider) => {
                bindings::serve(&extension_wrpc_client, provider).await
            }
            HttpServerProvider::Host(provider) => {
                bindings::serve(&extension_wrpc_client, provider).await
            }
        }
        .context("failed to serve extension capability interface")?;

        tasks.spawn(run_extension_serve_loop(
            extension_invocations,
            quit_rx,
            provider_id_for_task,
        ));

        let wrpc_client = Arc::new(
            self.provider_manager
                .produce_extension_wrpc_client(provider_id)
                .await?,
        );

        let config_bundle = self
            .complete_provider_configuration(
                provider_id,
                &config_names,
                None, // No claims for builtin providers
                &annotations,
                &wrpc_client,
            )
            .await?;

        // Spawn the periodic health checker
        tasks.spawn(check_health(
            Arc::clone(&wrpc_client),
            self.event_publisher.clone(),
            self.host_key.public_key(),
            provider_id.to_string(),
        ));

        // Spawn the config watcher task
        if let Some(bundle) = config_bundle {
            let provider_id_owned = provider_id.to_string();
            let rpc_nats = self.rpc_nats.clone();
            let lattice = self.host_config.lattice.clone();
            let host_id = Arc::from(self.host_key.public_key());
            tasks.spawn(async move {
                let config_bundle_arc = Arc::new(RwLock::new(bundle));
                let mut shutdown_rx = config_shutdown_rx;

                tokio::select! {
                    _ = watch_config(
                        rpc_nats,
                        config_bundle_arc,
                        lattice,
                        host_id,
                        provider_id_owned.clone(),
                    ) => {
                        trace!(provider_id = %provider_id_owned, "config watcher finished");
                    }
                    _ = shutdown_rx.recv() => {
                        trace!(provider_id = %provider_id_owned, "HTTP server provider received shutdown signal, config watcher stopping");
                    }
                }
            });
        }

        info!("Internal HTTP server provider started successfully");
        Ok(tasks)
    }
}

pub(crate) async fn listen<F, S>(address: SocketAddr, svc: F) -> anyhow::Result<JoinSet<()>>
where
    F: Fn(hyper::Request<hyper::body::Incoming>) -> S,
    F: Clone + Send + Sync + 'static,
    S: Future<Output = anyhow::Result<http::Response<wasmtime_wasi_http::body::HyperOutgoingBody>>>,
    S: Send + 'static,
{
    let socket = match &address {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
    };
    socket.set_reuseaddr(!cfg!(windows))?;
    socket.set_nodelay(true)?;
    socket.bind(address)?;
    let listener = socket.listen(8196)?;

    let svc = hyper::service::service_fn(svc);
    let srv = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    let srv = Arc::new(srv);

    let mut task = JoinSet::new();
    task.spawn(async move {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    error!(?err, "failed to accept HTTP server connection");
                    continue;
                }
            };
            let svc = svc.clone();
            tokio::spawn({
                let srv = Arc::clone(&srv);
                let svc = svc.clone();
                async move {
                    if let Err(err) = srv.serve_connection(TokioIo::new(stream), svc).await {
                        error!(?err, "failed to serve connection");
                    }
                }
            });
        }
    });

    Ok(task)
}

/// Run the extension interface serve loop for builtin providers.
async fn run_extension_serve_loop(
    extension_invocations: InvocationStreams,
    mut quit_rx: broadcast::Receiver<()>,
    provider_id: String,
) {
    use std::future::Future;
    use tokio::select;

    // same as in ['serve_provider_extension']
    fn map_invocation_stream(
        (instance, name, invocations): (
            &'static str,
            &'static str,
            Pin<
                Box<
                    dyn Stream<
                            Item = anyhow::Result<
                                Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                            >,
                        > + Send
                        + 'static,
                >,
            >,
        ),
    ) -> impl Stream<
        Item = (
            &'static str,
            &'static str,
            anyhow::Result<Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>>,
        ),
    > {
        invocations.map(move |res| (instance, name, res))
    }

    let mut invocations =
        stream::select_all(extension_invocations.into_iter().map(map_invocation_stream));
    let mut tasks = JoinSet::new();

    info!(
        provider_id,
        "Starting builtin provider extension serve loop"
    );

    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                match res {
                    std::result::Result::Ok(fut) => {
                        tasks.spawn(async move {
                            if let Err(err) = fut.await {
                                warn!(?err, instance, name, "failed to serve invocation");
                            }
                            trace!(instance, name, "successfully served invocation");
                        });
                    },
                    Err(err) => {
                        warn!(?err, instance, name, "failed to accept invocation");
                    }
                }
            },
            _ = quit_rx.recv() => {
                info!(provider_id, "Builtin provider received shutdown signal");
                // Graceful shutdown: wait for in-flight tasks
                let task_count = tasks.len();
                if task_count > 0 {
                    info!(provider_id, task_count, "Waiting for in-flight tasks");
                    while tasks.join_next().await.is_some() {}
                }
                info!(provider_id, "Builtin provider shutdown complete");
                return;
            }
        }
    }
}
