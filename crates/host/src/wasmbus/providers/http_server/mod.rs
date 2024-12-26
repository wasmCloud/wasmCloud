use core::net::SocketAddr;
use core::str::FromStr as _;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use nkeys::XKey;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinSet;
use tracing::{error, instrument};
use wasmcloud_core::InterfaceLinkDefinition;
use wasmcloud_provider_http_server::default_listen_address;
use wasmcloud_provider_sdk::provider::{
    handle_provider_commands, receive_link_for_provider, ProviderCommandReceivers,
};
use wasmcloud_provider_sdk::ProviderConnection;

pub(crate) mod address;
pub(crate) mod path;

/// Helper enum to allow for code reuse between different routing modes
enum HttpServerProvider {
    Address(address::Provider),
    Path(path::Provider),
}

impl crate::wasmbus::Host {
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn start_http_server_provider(
        &self,
        tasks: &mut JoinSet<()>,
        link_definitions: impl IntoIterator<Item = InterfaceLinkDefinition>,
        provider_xkey: XKey,
        host_config: HashMap<String, String>,
        provider_id: &str,
        host_id: &str,
    ) -> anyhow::Result<()> {
        let default_address = host_config
            .get("default_address")
            .map(|s| SocketAddr::from_str(s))
            .transpose()
            .context("failed to parse default_address")?
            .unwrap_or_else(default_listen_address);

        let provider = match host_config.get("routing_mode").map(String::as_str) {
            // Run provider in address mode by default
            Some("address") | None => HttpServerProvider::Address(address::Provider {
                address: default_address,
                components: Arc::clone(&self.components),
                links: Mutex::default(),
                host_id: Arc::from(host_id),
                lattice_id: Arc::clone(&self.host_config.lattice),
            }),
            // Run provider in path mode
            Some("path") => HttpServerProvider::Path(path::Provider {}),
            Some(other) => bail!("unknown routing_mode: {other}"),
        };

        let (quit_tx, quit_rx) = broadcast::channel(1);
        let commands = ProviderCommandReceivers::new(
            Arc::clone(&self.rpc_nats),
            &quit_tx,
            &self.host_config.lattice,
            provider_id,
            provider_id,
            host_id,
        )
        .await?;
        let conn = ProviderConnection::new(
            Arc::clone(&self.rpc_nats),
            Arc::from(provider_id),
            Arc::clone(&self.host_config.lattice),
            host_id.to_string(),
            host_config,
            provider_xkey,
            Arc::clone(&self.secrets_xkey),
        )
        .context("failed to establish provider connection")?;

        match provider {
            HttpServerProvider::Address(provider) => {
                for ld in link_definitions {
                    if let Err(e) = receive_link_for_provider(&provider, &conn, ld).await {
                        error!(
                            error = %e,
                            "failed to initialize link during provider startup",
                        );
                    }
                }

                tasks.spawn(async move {
                    handle_provider_commands(provider, &conn, quit_rx, quit_tx, commands).await
                });
            }
            HttpServerProvider::Path(provider) => {
                for ld in link_definitions {
                    if let Err(e) = receive_link_for_provider(&provider, &conn, ld).await {
                        error!(
                            error = %e,
                            "failed to initialize link during provider startup",
                        );
                    }
                }

                tasks.spawn(async move {
                    handle_provider_commands(provider, &conn, quit_rx, quit_tx, commands).await
                });
            }
        }
        Ok(())
    }
}
