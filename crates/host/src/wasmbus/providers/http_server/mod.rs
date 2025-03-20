use core::net::SocketAddr;
use core::str::FromStr as _;

use std::future::Future;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use hyper_util::rt::{TokioExecutor, TokioIo};
use nkeys::XKey;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinSet;
use tracing::{error, instrument};
use wasmcloud_core::{http::default_listen_address, HostData};
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
        host_data: HostData,
        provider_xkey: XKey,
        provider_id: &str,
    ) -> anyhow::Result<JoinSet<()>> {
        let host_id = self.host_key.public_key();
        let default_address = host_data
            .config
            .get("default_address")
            .map(|s| SocketAddr::from_str(s))
            .transpose()
            .context("failed to parse default_address")?
            .unwrap_or_else(default_listen_address);

        let provider = match host_data.config.get("routing_mode").map(String::as_str) {
            // Run provider in address mode by default
            Some("address") | None => HttpServerProvider::Address(address::Provider {
                address: default_address,
                components: Arc::clone(&self.components),
                links: Mutex::default(),
                host_id: Arc::from(host_id.as_str()),
                lattice_id: Arc::clone(&self.host_config.lattice),
            }),
            // Run provider in path mode
            Some("path") => HttpServerProvider::Path(
                path::Provider::new(
                    default_address,
                    Arc::clone(&self.components),
                    Arc::from(host_id.as_str()),
                    Arc::clone(&self.host_config.lattice),
                )
                .await?,
            ),
            Some(other) => bail!("unknown routing_mode: {other}"),
        };

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

        let mut tasks = JoinSet::new();
        match provider {
            HttpServerProvider::Address(provider) => {
                for ld in host_data.link_definitions {
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
                for ld in host_data.link_definitions {
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
