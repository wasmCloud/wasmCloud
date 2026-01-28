//! Implementation of the `wrpc:http/incoming-handler` provider in address mode
//!
//! This provider listens on a new address for each component that it links to.

use core::str::FromStr as _;
use core::time::Duration;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use axum::extract;
use axum::handler::Handler;
use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument};
use wasmcloud_core::http::{default_listen_address, load_settings, ServiceSettings};
use wasmcloud_provider_sdk::core::LinkName;
use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::{get_connection, HostData, LinkConfig, LinkDeleteInfo, Provider};

use crate::{build_request, get_cors_layer, get_tcp_listener, invoke_component, axum_deprecated};

/// Lookup for handlers by socket
///
/// Indexed first by socket address to more easily detect duplicates,
/// with the http server stored, along with a list (order matters) of components that were registered
type HandlerLookup =
    HashMap<SocketAddr, (Arc<HttpServerCore>, Vec<(Arc<str>, Arc<str>, WrpcClient)>)>;

/// `wrpc:http/incoming-handler` provider implementation in address mode
#[derive(Clone)]
pub struct HttpServerProvider {
    default_address: SocketAddr,

    /// Lookup of components that handle requests {addr -> (server, (component id, link name))}
    handlers_by_socket: Arc<RwLock<HandlerLookup>>,

    /// Sockets that are relevant to a given link name
    ///
    /// This structure is generally used as a look up into `handlers_by_socket`
    sockets_by_link_name: Arc<RwLock<HashMap<LinkName, SocketAddr>>>,
}

impl Default for HttpServerProvider {
    fn default() -> Self {
        Self {
            default_address: default_listen_address(),
            handlers_by_socket: Arc::default(),
            sockets_by_link_name: Arc::default(),
        }
    }
}

impl HttpServerProvider {
    /// Create a new instance of the HTTP server provider
    pub fn new(host_data: &HostData) -> anyhow::Result<Self> {
        let default_address = host_data
            .config
            .get("default_address")
            .map(|s| SocketAddr::from_str(s))
            .transpose()
            .context("failed to parse default_address")?
            .unwrap_or_else(default_listen_address);

        Ok(Self {
            default_address,
            handlers_by_socket: Arc::default(),
            sockets_by_link_name: Arc::default(),
        })
    }
}

impl Provider for HttpServerProvider {
    /// This is called when the HTTP server provider is linked to a component
    ///
    /// This HTTP server mode will listen on a new address for each component that it links to.
    async fn receive_link_config_as_source(
        &self,
        link_config: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let settings = match load_settings(Some(self.default_address), link_config.config)
            .context("httpserver failed to load settings for component")
        {
            Ok(settings) => settings,
            Err(e) => {
                error!(
                    config = ?link_config.config,
                    "httpserver failed to load settings for component: {}", e.to_string()
                );
                bail!(e);
            }
        };

        let wrpc = get_connection()
            .get_wrpc_client(link_config.target_id)
            .await
            .context("failed to construct wRPC client")?;
        let component_meta = (
            Arc::from(link_config.target_id),
            Arc::from(link_config.link_name),
            wrpc,
        );
        let mut sockets_by_link_name = self.sockets_by_link_name.write().await;
        let mut handlers_by_socket = self.handlers_by_socket.write().await;

        match sockets_by_link_name.entry(link_config.link_name.to_string()) {
            // If a mapping already exists, and the stored address is different, disallow overwriting
            std::collections::hash_map::Entry::Occupied(v) => {
                bail!(
                    "an address mapping for address [{}] the link [{}] already exists, overwriting links is not currently supported",
                    v.get().ip(),
                    link_config.link_name,
                )
            }
            // If a mapping does exist, we can create a new mapping for the address
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(settings.address);
            }
        }

        match handlers_by_socket.entry(settings.address) {
            // If handlers already exist for the address, add the newly linked component
            //
            // NOTE: only components at the head of the list are served requests
            std::collections::hash_map::Entry::Occupied(mut v) => {
                v.get_mut().1.push(component_meta);
            }
            // If a handler does not already exist, make a new server and insert
            std::collections::hash_map::Entry::Vacant(v) => {
                // Start a server instance that calls the given component
                let http_server = match HttpServerCore::new(
                    Arc::new(settings),
                    link_config.target_id,
                    self.handlers_by_socket.clone(),
                )
                .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        error!("failed to start listener for component: {e:?}");
                        bail!(e);
                    }
                };
                v.insert((Arc::new(http_server), vec![component_meta]));
            }
        }

        Ok(())
    }

    /// Handle notification that a link is dropped - stop the http listener
    #[instrument(level = "info", skip_all, fields(target_id = info.get_target_id()))]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_target_id();
        let link_name = info.get_link_name();

        // Retrieve the thing by link name
        let mut sockets_by_link_name = self.sockets_by_link_name.write().await;
        if let Some(addr) = sockets_by_link_name.get(link_name) {
            let mut handlers_by_socket = self.handlers_by_socket.write().await;
            if let Some((server, component_metas)) = handlers_by_socket.get_mut(addr) {
                // If the component id & link name pair is present, remove it
                if let Some(idx) = component_metas
                    .iter()
                    .position(|(c, l, ..)| c.as_ref() == component_id && l.as_ref() == link_name)
                {
                    component_metas.remove(idx);
                }

                // If the component was the last one, we can remove the server
                if component_metas.is_empty() {
                    info!(
                        address = addr.to_string(),
                        "last component removed for address, shutting down server"
                    );
                    server.handle.shutdown();
                    handlers_by_socket.remove(addr);
                    sockets_by_link_name.remove(link_name);
                }
            }
        }

        Ok(())
    }

    /// Handle shutdown request by shutting down all the http server threads
    async fn shutdown(&self) -> anyhow::Result<()> {
        // Empty the component link data and stop all servers
        self.sockets_by_link_name.write().await.clear();
        self.handlers_by_socket.write().await.clear();
        Ok(())
    }
}

#[derive(Clone)]
struct RequestContext {
    /// Address of the server, used for handler lookup
    server_address: SocketAddr,
    /// Settings that can be
    settings: Arc<ServiceSettings>,
    /// HTTP scheme
    scheme: http::uri::Scheme,
    /// Handlers for components
    handlers_by_socket: Arc<RwLock<HandlerLookup>>,
}

/// Handle an HTTP request by invoking the target component as configured in the listener
#[instrument(level = "debug", skip(settings, handlers_by_socket))]
async fn handle_request(
    extract::State(RequestContext {
        server_address,
        settings,
        scheme,
        handlers_by_socket,
    }): extract::State<RequestContext>,
    axum_deprecated::Host(authority): axum_deprecated::Host,
    request: extract::Request,
) -> impl axum::response::IntoResponse {
    let (component_id, wrpc) = {
        let Some((component_id, wrpc)) = handlers_by_socket
            .read()
            .await
            .get(&server_address)
            .and_then(|v| v.1.first())
            .map(|(component_id, _, wrpc)| (Arc::clone(component_id), wrpc.clone()))
        else {
            return Err((
                http::StatusCode::INTERNAL_SERVER_ERROR,
                "no targets for HTTP request",
            ))?;
        };
        (component_id, wrpc)
    };

    let timeout = settings.timeout_ms.map(Duration::from_millis);
    let req = build_request(request, scheme, authority, &settings).map_err(|err| *err)?;
    axum::response::Result::<_, axum::response::ErrorResponse>::Ok(
        invoke_component(
            &wrpc,
            &component_id,
            req,
            timeout,
            settings.cache_control.as_ref(),
        )
        .await,
    )
}

/// An asynchronous `wrpc:http/incoming-handler` with support for CORS and TLS
#[derive(Debug)]
pub struct HttpServerCore {
    /// The handle to the server handling incoming requests
    handle: axum_server::Handle,
    /// The asynchronous task running the server
    task: tokio::task::JoinHandle<()>,
}

impl HttpServerCore {
    #[instrument(skip(handlers_by_socket))]
    pub async fn new(
        settings: Arc<ServiceSettings>,
        target: &str,
        handlers_by_socket: Arc<RwLock<HandlerLookup>>,
    ) -> anyhow::Result<Self> {
        let addr = settings.address;
        info!(
            %addr,
            component_id = target,
            "httpserver starting listener for target",
        );
        let cors = get_cors_layer(&settings)?;
        let service = handle_request.layer(cors);
        let handle = axum_server::Handle::new();
        let listener = get_tcp_listener(&settings)
            .with_context(|| format!("failed to create listener (is [{addr}] already in use?)"))?;

        let target = target.to_owned();
        let task_handle = handle.clone();
        let task = if let (Some(crt), Some(key)) =
            (&settings.tls_cert_file, &settings.tls_priv_key_file)
        {
            debug!(?addr, "bind HTTPS listener");
            let tls = RustlsConfig::from_pem_file(crt, key)
                .await
                .context("failed to construct TLS config")?;

            let srv = axum_server::from_tcp_rustls(listener, tls);
            tokio::spawn(async move {
                if let Err(e) = srv
                    .handle(task_handle)
                    .serve(
                        service
                            .with_state(RequestContext {
                                server_address: addr,
                                settings,
                                scheme: http::uri::Scheme::HTTPS,
                                handlers_by_socket,
                            })
                            .into_make_service(),
                    )
                    .await
                {
                    error!(error = %e, component_id = target, "failed to serve HTTPS for component");
                }
            })
        } else {
            debug!(?addr, "bind HTTP listener");

            let mut srv = axum_server::from_tcp(listener);
            srv.http_builder().http1().keep_alive(false);
            tokio::spawn(async move {
                if let Err(e) = srv
                    .handle(task_handle)
                    .serve(
                        service
                            .with_state(RequestContext {
                                server_address: addr,
                                settings,
                                scheme: http::uri::Scheme::HTTP,
                                handlers_by_socket,
                            })
                            .into_make_service(),
                    )
                    .await
                {
                    error!(error = %e, component_id = target, "failed to serve HTTP for component");
                }
            })
        };

        Ok(Self { handle, task })
    }
}

impl Drop for HttpServerCore {
    /// Drop the client connection. Does not block or fail if the client has already been closed.
    fn drop(&mut self) {
        self.handle.shutdown();
        self.task.abort();
    }
}
