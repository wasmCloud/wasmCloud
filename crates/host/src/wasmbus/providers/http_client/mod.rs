use anyhow::Context as _;
use futures::{stream, StreamExt as _};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinSet;
use tokio::{pin, select};
use tracing::{info, trace, warn};

use wasmcloud_provider_sdk::ProviderConnection;
use wrpc_interface_http::ServeHttp;

// Re-export the main HTTP client provider implementation
pub(crate) mod provider;

use wasmcloud_core::http_client::DEFAULT_IDLE_TIMEOUT;

use crate::bindings;
use crate::wasmbus::providers::{check_health, watch_config};

/// Default timeout for graceful shutdown - wait for in-flight tasks to complete
const GRACEFUL_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl crate::wasmbus::Host {
    /// Initializes and starts the internal HTTP client provider.
    ///
    /// The provider starts with default TLS configuration and will receive
    /// actual configuration via the update_base_config wRPC call after binding.
    pub(crate) async fn start_http_client_provider(
        self: Arc<Self>,
        provider_id: &str,
        config_names: Vec<String>,
        annotations: BTreeMap<String, String>,
    ) -> anyhow::Result<JoinSet<()>> {
        info!(
            "Starting internal HTTP client provider with ID: {}",
            provider_id
        );

        let (quit_tx, quit_rx) = broadcast::channel(1);
        // Get a separate receiver for the config watcher shutdown
        let config_shutdown_rx = quit_tx.subscribe();

        let provider = provider::HttpClientProvider::new(DEFAULT_IDLE_TIMEOUT, quit_tx).await?;

        let mut tasks = JoinSet::new();

        let conn = ProviderConnection::new(
            Arc::clone(&self.rpc_nats),
            Arc::from(provider_id),
            Arc::clone(&self.host_config.lattice),
            self.host_key.public_key(),
        )
        .context("failed to establish provider connection")?;

        // Construct wRPC client for main provider exports (standard lattice subject)
        // This is where components will invoke the HTTP outgoing-handler interface
        let main_wrpc_client = conn
            .get_wrpc_client(&conn.provider_id)
            .await
            .context("failed to create main wRPC client")?;

        // Construct wRPC client for extension capability interface (host-specific subject)
        // This is where the host will invoke manageable/configurable interfaces
        let extension_wrpc_client = conn
            .get_wrpc_extension_serve_client_custom(None)
            .await
            .context("failed to create extension wRPC client")?;

        // Serve the HTTP outgoing-handler interface on the main lattice subject
        let http_invocations =
            wrpc_interface_http::bindings::exports::wrpc::http::outgoing_handler::serve_interface(
                &main_wrpc_client,
                ServeHttp(provider.clone()),
            )
            .await
            .context("failed to serve HTTP outgoing-handler interface")?;

        // Serve extension interfaces (manageable, configurable) on the host-specific subject
        let extension_invocations = bindings::serve(&extension_wrpc_client, provider)
            .await
            .context("failed to serve extension capability interface")?;

        // Spawn the main dispatch loop task
        let provider_id_for_task = provider_id.to_string();
        tasks.spawn(async move {
            info!("Starting internal HTTP client provider dispatch loop");

            // Combine all invocation streams
            let mut invocations = stream::select_all(
                http_invocations
                    .into_iter()
                    .chain(extension_invocations.into_iter())
                    .map(|(instance, name, stream)| stream.map(move |res| (instance, name, res))),
            );

            let mut quit_rx = quit_rx;
            let shutdown = async {
                let _ = quit_rx.recv().await;
                info!("HTTP client provider received shutdown signal, terminating.");
            };
            pin!(shutdown);

            let mut handler_tasks = JoinSet::new();

            loop {
                select! {
                    Some((instance, name, res)) = invocations.next() => {
                        match res {
                            Ok(fut) => {
                                handler_tasks.spawn(async move {
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
                    () = &mut shutdown => {
                        // Graceful shutdown: stop accepting new invocations, wait for in-flight to complete
                        let task_count = handler_tasks.len();
                        if task_count > 0 {
                            info!("Shutdown requested, waiting for {} in-flight tasks", task_count);

                            // Wait for tasks with a timeout
                            let shutdown_result = tokio::time::timeout(
                                GRACEFUL_SHUTDOWN_TIMEOUT,
                                async {
                                    while handler_tasks.join_next().await.is_some() {}
                                }
                            ).await;

                            match shutdown_result {
                                Ok(()) => {
                                    info!("Shutdown completed gracefully");
                                }
                                Err(_) => {
                                    let remaining = handler_tasks.len();
                                    warn!(remaining, "Graceful shutdown timeout, aborting remaining tasks");
                                    handler_tasks.abort_all();
                                    // Wait for aborted tasks to be cleaned up
                                    while handler_tasks.join_next().await.is_some() {}
                                }
                            }
                        }

                        info!(provider_id = %provider_id_for_task, "HTTP client provider shutdown complete");
                        return;
                    }
                }
            }
        });

        // Create wRPC client to communicate with the provider we just started
        let wrpc_client = Arc::new(
            self.provider_manager
                .produce_extension_wrpc_client(provider_id)
                .await?,
        );

        // Perform the full health-check, bind, and configuration flow.
        // This will call update_base_config which sets configured=true.
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

        // Spawn the config watcher task with proper async shutdown signaling if provider supports configuration
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
                        trace!(provider_id = %provider_id_owned, "builtin provider received shutdown signal, config watcher stopping");
                    }
                }
            });
        }

        info!("Internal HTTP client provider started successfully");
        Ok(tasks)
    }
}
