use anyhow::Context as _;
use futures::StreamExt as _;
use nkeys::XKey;
use rustls_pemfile;
use std::io::BufReader;
use std::sync::Arc;
use tokio::select;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};
use webpki_roots;

use wasmcloud_core::HostData;
use wasmcloud_provider_sdk::{
    provider::{handle_provider_commands, receive_link_for_provider, ProviderCommandReceivers},
    ProviderConnection,
};
use wrpc_interface_http::ServeHttp;

// Re-export the main HTTP client provider implementation
pub(crate) mod provider;

// Common features shared with external HTTP client provider
use wasmcloud_core::http_client::{
    DEFAULT_IDLE_TIMEOUT, LOAD_NATIVE_CERTS, LOAD_WEBPKI_CERTS, SSL_CERTS_FILE,
};

impl crate::wasmbus::Host {
    /// Initializes and starts the internal HTTP client provider.
    ///
    /// This method is called by the wasmCloud host to create and start the built-in
    /// HTTP client provider. It sets up the provider with the host's configuration,
    /// establishes the necessary NATS connections, and starts the provider's command
    /// handling tasks.
    ///
    /// # Arguments
    ///
    /// * `host_data` - Configuration data from the host
    /// * `provider_xkey` - The provider's signing key for secure communications
    /// * `provider_id` - Unique identifier for this provider instance
    ///
    /// # Returns
    ///
    /// A JoinSet containing the provider's background tasks, or an error if initialization fails
    pub(crate) async fn start_http_client_provider(
        &self,
        host_data: HostData,
        provider_xkey: XKey,
        provider_id: &str,
    ) -> anyhow::Result<JoinSet<()>> {
        info!("Starting HTTP client provider with ID: {}", provider_id);
        let host_id = self.host_key.public_key();

        // Initialize TLS connector based on configuration
        let tls = if host_data.config.is_empty() {
            debug!("Using default TLS connector");
            wasmcloud_provider_sdk::core::tls::DEFAULT_RUSTLS_CONNECTOR.clone()
        } else {
            debug!("Configuring custom TLS connector");
            let mut ca = rustls::RootCertStore::empty();

            // Load native certificates if configured
            if host_data
                .config
                .get(LOAD_NATIVE_CERTS)
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(true)
            {
                let (added, ignored) = ca.add_parsable_certificates(
                    wasmcloud_provider_sdk::core::tls::NATIVE_ROOTS
                        .iter()
                        .cloned(),
                );
                debug!(added, ignored, "loaded native root certificate store");
            }

            // Load Mozilla trusted root certificates if configured
            if host_data
                .config
                .get(LOAD_WEBPKI_CERTS)
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(true)
            {
                ca.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                debug!("loaded webpki root certificate store");
            }

            // Load additional certificates from file if specified
            if let Some(file_path) = host_data.config.get(SSL_CERTS_FILE) {
                let f = std::fs::File::open(file_path)?;
                let mut reader = BufReader::new(f);
                let certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
                let (added, ignored) = ca.add_parsable_certificates(certs);
                debug!(
                    added,
                    ignored, "added additional root certificates from file"
                );
            }

            tokio_rustls::TlsConnector::from(Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(ca)
                    .with_no_client_auth(),
            ))
        };

        // Create provider instance
        debug!("Creating HTTP client provider instance");
        let provider = provider::HttpClientProvider::new(tls, DEFAULT_IDLE_TIMEOUT).await?;

        let mut tasks = JoinSet::new();

        debug!("Setting up provider command receivers");
        let (quit_tx, quit_rx) = broadcast::channel(1);
        let commands = ProviderCommandReceivers::new(
            Arc::clone(&self.rpc_nats),
            &quit_tx,
            &self.host_config.lattice,
            provider_id,
            provider_id,
            &host_id,
        )
        .await?;

        debug!("Creating provider connection");
        let conn = ProviderConnection::new(
            Arc::clone(&self.rpc_nats),
            Arc::from(provider_id),
            Arc::clone(&self.host_config.lattice),
            host_id.to_string(),
            host_data.config,
            provider_xkey,
            Arc::clone(&self.secrets_xkey),
        )
        .context("failed to establish provider connection")?;

        debug!(
            target: "http_client::connection",
            provider_id = %provider_id,
            lattice = %self.host_config.lattice,
            "Provider connection created"
        );

        // Process link definitions with enhanced logging
        for ld in host_data.link_definitions {
            debug!(
                target: "http_client::link",
                link_name = %ld.name,
                source_id = %ld.source_id,
                target = %ld.target,
                interfaces = ?ld.interfaces,
                "Processing link definitions"
            );
            if let Err(err) = receive_link_for_provider(&provider, &conn, ld.clone()).await {
                error!(
                    target: "http_client::link",
                    error = %err,
                    "Failed to initialize link during provider startup"
                );
            } else {
                debug!(
                    target: "http_client::link",
                    instance = "wasi:http/outgoing-handler",
                    link_name = %ld.name,
                    target = %provider_id,
                    "Successfully initialized link"
                );
            }
        }

        let provider_clone = provider.clone();
        let conn_clone = conn.clone();

        info!("Starting provider command handler");
        tasks.spawn(async move {
            handle_provider_commands(provider, &conn, quit_rx, quit_tx, commands).await
        });

        // Start the HTTP client interface handler
        tasks.spawn(async move {
            debug!("Setting up wrpc interface");
            let wrpc = match conn_clone.get_wrpc_client(conn_clone.provider_key()).await {
                Ok(wrpc) => wrpc,
                Err(err) => {
                    error!("Failed to get wRPC client: {}", err);
                    return;
                }
            };

            let [(_, _, mut invocations)] = match wrpc_interface_http::bindings::exports::wrpc::http::outgoing_handler::serve_interface(
                &wrpc,
                ServeHttp(provider_clone.clone()),
            ).await {
                Ok(interfaces) => interfaces,
                Err(err) => {
                    error!("Failed to serve exports: {}", err);
                    return;
                }
            };

            info!("HTTP client provider ready to handle requests");
            let mut tasks = JoinSet::new();

            loop {
                select! {
                    Some(res) = invocations.next() => {
                        match res {
                            Ok(fut) => {
                                tasks.spawn(async move {
                                    if let Err(err) = fut.await {
                                        warn!(?err, "failed to serve invocation");
                                    }
                                });
                            },
                            Err(err) => {
                                warn!(?err, "failed to accept invocation");
                            }
                        }
                    }
                }
            }
        });

        info!("HTTP client provider started successfully");
        Ok(tasks)
    }
}
