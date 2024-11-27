use core::net::SocketAddr;
use core::str::FromStr as _;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use http::header::HOST;
use http::uri::Scheme;
use http::Uri;
use http_body_util::BodyExt as _;
use hyper_util::rt::{TokioExecutor, TokioIo};
use nkeys::XKey;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::{error, instrument};
use wasmcloud_core::InterfaceLinkDefinition;
use wasmcloud_provider_http_server::{default_listen_address, load_settings, ServiceSettings};
use wasmcloud_provider_sdk::provider::{
    handle_provider_commands, receive_link_for_provider, ProviderCommandReceivers,
};
use wasmcloud_provider_sdk::{LinkConfig, LinkDeleteInfo, ProviderConnection};
use wasmcloud_tracing::KeyValue;
use wrpc_interface_http::ServeIncomingHandlerWasmtime as _;

use crate::wasmbus::Component;

struct Provider {
    address: SocketAddr,
    components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
    links: Mutex<HashMap<Arc<str>, HashMap<Box<str>, JoinSet<()>>>>,
    lattice_id: Arc<str>,
    host_id: Arc<str>,
}

impl wasmcloud_provider_sdk::Provider for Provider {
    #[instrument(level = "debug", skip_all)]
    async fn receive_link_config_as_source(
        &self,
        LinkConfig {
            target_id,
            config,
            link_name,
            ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let ServiceSettings { address, .. } =
            load_settings(Some(self.address), config).context("failed to load settings")?;

        let socket = match &address {
            SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
            SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
        };
        socket.set_reuseaddr(!cfg!(windows))?;
        socket.set_nodelay(true)?;
        socket.bind(address)?;
        let listener = socket.listen(8196)?;

        let mut tasks = JoinSet::new();
        let target_id: Arc<str> = Arc::from(target_id);
        let svc = hyper::service::service_fn({
            let target_id = Arc::clone(&target_id);
            let lattice_id = Arc::clone(&self.lattice_id);
            let host_id = Arc::clone(&self.host_id);
            let components = Arc::clone(&self.components);
            move |req: hyper::Request<hyper::body::Incoming>| {
                let target_id = Arc::clone(&target_id);
                let lattice_id = Arc::clone(&lattice_id);
                let host_id = Arc::clone(&host_id);
                let components = Arc::clone(&components);
                async move {
                    let component = {
                        let components = components.read().await;
                        let component = components
                            .get(target_id.as_ref())
                            .context("linked component not found")?;
                        Arc::clone(component)
                    };
                    let (
                        http::request::Parts {
                            method,
                            uri,
                            headers,
                            ..
                        },
                        body,
                    ) = req.into_parts();
                    let http::uri::Parts {
                        scheme,
                        authority,
                        path_and_query,
                        ..
                    } = uri.into_parts();
                    let mut uri = Uri::builder().scheme(scheme.unwrap_or(Scheme::HTTP));
                    if let Some(authority) = authority {
                        uri = uri.authority(authority);
                    } else if let Some(authority) = headers.get("X-Forwarded-Host") {
                        uri = uri.authority(authority.as_bytes());
                    } else if let Some(authority) = headers.get(HOST) {
                        uri = uri.authority(authority.as_bytes());
                    }
                    if let Some(path_and_query) = path_and_query {
                        uri = uri.path_and_query(path_and_query)
                    };
                    let uri = uri.build().context("invalid URI")?;
                    let mut req = http::Request::builder().method(method);
                    *req.headers_mut().expect("headers missing") = headers;

                    let req = req
                        .uri(uri)
                        .body(
                            body.map_err(wasmtime_wasi_http::hyper_response_error)
                                .boxed(),
                        )
                        .context("invalid request")?;
                    let _permit = component
                        .permits
                        .acquire()
                        .await
                        .context("failed to acquire execution permit")?;
                    let res = component
                        .instantiate(component.handler.copy_for_new(), component.events.clone())
                        .handle(
                            (
                                Instant::now(),
                                vec![
                                    KeyValue::new(
                                        "component.ref",
                                        Arc::clone(&component.image_reference),
                                    ),
                                    KeyValue::new("lattice", Arc::clone(&lattice_id)),
                                    KeyValue::new("host", Arc::clone(&host_id)),
                                ],
                            ),
                            req,
                        )
                        .await?;
                    let res = res?;
                    anyhow::Ok(res)
                }
            }
        });

        let srv = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
        let srv = Arc::new(srv);

        tasks.spawn(async move {
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
        self.links
            .lock()
            .await
            .entry(target_id)
            .or_default()
            .insert(link_name.into(), tasks);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let target_id = info.get_target_id();
        let link_name = info.get_link_name();
        self.links
            .lock()
            .await
            .get_mut(target_id)
            .map(|links| links.remove(link_name));
        Ok(())
    }
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
        match host_config.get("routing_mode").map(String::as_str) {
            // Run provider in address mode by default
            Some("address") | None => {}
            // Run provider in path mode
            Some("path") => bail!("path mode not supported by builtin yet"),
            Some(other) => bail!("unknown routing_mode: {other}"),
        };
        let default_address = host_config
            .get("default_address")
            .map(|s| SocketAddr::from_str(s))
            .transpose()
            .context("failed to parse default_address")?
            .unwrap_or_else(default_listen_address);

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
        let provider = Provider {
            address: default_address,
            components: Arc::clone(&self.components),
            links: Mutex::default(),
            host_id: Arc::from(host_id),
            lattice_id: Arc::clone(&self.host_config.lattice),
        };
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
        Ok(())
    }
}
