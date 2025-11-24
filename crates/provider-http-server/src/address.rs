//! Implementation of the `wrpc:http/incoming-handler` provider in address mode
//!
//! This provider listens on a new address for each component that it links to.

use core::str::FromStr as _;
use core::time::Duration;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::bindings::exports::wrpc::extension::{
    configurable::{self, InterfaceConfig},
    manageable,
};
use anyhow::Context as _;
use axum::extract;
use axum::handler::Handler;
use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument};
use wasmcloud_core::{
    http::{default_listen_address, load_settings, ServiceSettings},
    LinkName,
};
use wasmcloud_provider_sdk::{
    get_connection,
    provider::WrpcClient,
    types::{BindRequest, BindResponse, HealthCheckResponse},
    Context,
};

use crate::{build_request, get_cors_layer, get_tcp_listener, invoke_component};

/// Lookup for handlers by socket
///
/// Indexed first by socket address to more easily detect duplicates,
/// with the http server stored, along with a list (order matters) of components that were registered
type HandlerLookup =
    HashMap<SocketAddr, (Arc<HttpServerCore>, Vec<(Arc<str>, Arc<str>, WrpcClient)>)>;

/// `wrpc:http/incoming-handler` provider implementation in address mode
#[derive(Clone)]
pub struct HttpServerProvider {
    default_address: Arc<RwLock<SocketAddr>>,

    /// Lookup of components that handle requests {addr -> (server, (component id, link name))}
    handlers_by_socket: Arc<RwLock<HandlerLookup>>,

    /// Sockets that are relevant to a given link name
    ///
    /// This structure is generally used as a look up into `handlers_by_socket`
    sockets_by_link_name: Arc<RwLock<HashMap<LinkName, SocketAddr>>>,

    /// Channel to signal provider shutdown
    quit_tx: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl HttpServerProvider {
    pub fn new(quit_tx: tokio::sync::broadcast::Sender<()>) -> Self {
        Self {
            default_address: Arc::new(RwLock::new(default_listen_address())),
            handlers_by_socket: Arc::default(),
            sockets_by_link_name: Arc::default(),
            quit_tx: Arc::new(quit_tx),
        }
    }
}

impl configurable::Handler<Option<Context>> for HttpServerProvider {
    async fn update_base_config(
        &self,
        _cx: Option<Context>,
        config: wasmcloud_provider_sdk::types::BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config: HashMap<String, String> = config.config.into_iter().collect();

        let default_address = config
            .get("default_address")
            .map(|s| SocketAddr::from_str(s))
            .transpose()
            .context("failed to parse default_address")?
            .unwrap_or_else(default_listen_address);

        *self.default_address.write().await = default_address;
        Ok(Ok(()))
    }

    async fn update_interface_export_config(
        &self,
        _cx: Option<Context>,
        _source_id: String,
        _link_name: String,
        _link_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    async fn update_interface_import_config(
        &self,
        _cx: Option<Context>,
        target_id: String,
        link_name: String,
        interface_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config: HashMap<String, String> = interface_config.config.into_iter().collect();

        let settings = match load_settings(Some(*self.default_address.read().await), &config) {
            Result::Ok(settings) => settings,
            Result::Err(e) => {
                error!(
                    ?config,
                    "httpserver failed to load settings for component: {}",
                    e.to_string()
                );
                return Ok(Err(format!(
                    "httpserver failed to load settings for component: {e}"
                )));
            }
        };

        let wrpc = match get_connection().get_wrpc_client(&target_id).await {
            Result::Ok(wrpc) => wrpc,
            Result::Err(e) => {
                return Ok(Err(format!("failed to construct wRPC client: {e}")));
            }
        };
        let component_meta = (
            Arc::from(target_id.clone()),
            Arc::from(link_name.clone()),
            wrpc,
        );
        let mut sockets_by_link_name = self.sockets_by_link_name.write().await;
        let mut handlers_by_socket = self.handlers_by_socket.write().await;

        match sockets_by_link_name.entry(link_name.to_string()) {
            // If a mapping already exists, and the stored address is different, disallow overwriting
            std::collections::hash_map::Entry::Occupied(v) => {
                return Ok(Err(format!(
                    "an address mapping for address [{}] the link [{}] already exists, overwriting links is not currently supported",
                    v.get().ip(),
                    link_name,
                )));
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
                    &target_id,
                    self.handlers_by_socket.clone(),
                )
                .await
                {
                    Result::Ok(s) => s,
                    Result::Err(e) => {
                        error!("failed to start listener for component: {e:?}");
                        return Ok(Err(format!("failed to start listener for component: {e}")));
                    }
                };
                v.insert((Arc::new(http_server), vec![component_meta]));
            }
        }

        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(target_id))]
    async fn delete_interface_import_config(
        &self,
        cx: Option<Context>,
        target_id: String,
        link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        let Some(cx) = cx else {
            return Ok(Err("missing context".to_string()));
        };
        debug!(
            source = cx.component,
            target = target_id,
            link = link_name,
            "deleting http host link"
        );

        // Retrieve the thing by link name
        let mut sockets_by_link_name = self.sockets_by_link_name.write().await;
        if let Some(addr) = sockets_by_link_name.get(&link_name) {
            let mut handlers_by_socket = self.handlers_by_socket.write().await;
            if let Some((server, component_metas)) = handlers_by_socket.get_mut(addr) {
                // If the component id & link name pair is present, remove it
                if let Some(idx) = component_metas
                    .iter()
                    .position(|(c, l, ..)| c.as_ref() == target_id && l.as_ref() == link_name)
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
                    sockets_by_link_name.remove(&link_name);
                }
            }
        }

        Ok(Ok(()))
    }

    async fn delete_interface_export_config(
        &self,
        _cx: Option<Context>,
        _source_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }
}

impl manageable::Handler<Option<Context>> for HttpServerProvider {
    async fn bind(
        &self,
        _cx: Option<Context>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        Ok(Ok(BindResponse {
            identity_token: None,
            provider_xkey: Some(get_connection().provider_xkey.public_key().into()),
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<Context>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        Ok(Ok(HealthCheckResponse {
            healthy: true,
            message: Some("OK".to_string()),
        }))
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self, _cx: Option<Context>) -> anyhow::Result<Result<(), String>> {
        // Empty the component link data and stop all servers
        self.sockets_by_link_name.write().await.clear();
        self.handlers_by_socket.write().await.clear();

        // Signal the provider to shut down
        let _ = self.quit_tx.send(());
        Ok(Ok(()))
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
    axum_extra::extract::Host(authority): axum_extra::extract::Host,
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
