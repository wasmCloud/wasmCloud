//! This module contains the implementation of the `wrpc:http/incoming-handler` provider in host-based mode.
//!
//! In host-based mode, the HTTP server listens on a single address and routes requests to different components
//! based on the host of the request.

use core::time::Duration;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use crate::bindings::exports::wrpc::extension::{
    configurable::{self, InterfaceConfig},
    manageable,
};
use axum::extract;
use axum::handler::Handler;
use axum_server::tls_rustls::RustlsConfig;
use axum_server::Handle;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, instrument};
use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::Context;
use wasmcloud_provider_sdk::{
    get_connection,
    types::{BindRequest, BindResponse, HealthCheckResponse},
};

use crate::{
    build_request, get_cors_layer, get_tcp_listener, invoke_component, load_settings,
    ServiceSettings,
};

/// This struct holds both the forward and reverse mappings for host-based routing
/// so that they can be modified by just acquiring a single lock in the [`HttpServerProvider`]
#[derive(Default)]
struct Router {
    /// Lookup from a host to the component ID that is handling that host
    hosts: HashMap<Arc<str>, (Arc<str>, WrpcClient)>,
    /// Reverse lookup to find the host for a (component,link_name) pair
    components: HashMap<(Arc<str>, Arc<str>), Arc<str>>,
    /// Header to match for host-based routing
    header: String,
}

/// Holds the server handle and task for graceful shutdown
struct ServerState {
    /// [`Handle`] to the server task for graceful shutdown
    handle: Handle,
    /// Task handle for the server task
    task: JoinHandle<()>,
}

impl ServerState {
    /// Shutdown the server gracefully
    fn shutdown(&self) {
        self.handle.shutdown();
        self.task.abort();
    }
}

/// `wrpc:http/incoming-handler` provider implementation with host-based routing
#[derive(Clone)]
pub struct HttpServerProvider {
    /// Struct that holds the routing information based on host/component_id
    router: Arc<RwLock<Router>>,
    /// Server state (handle + task), None if server not yet started
    server_state: Arc<RwLock<Option<ServerState>>>,
    /// Channel to signal provider shutdown
    quit_tx: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl HttpServerProvider {
    pub fn new(quit_tx: tokio::sync::broadcast::Sender<()>) -> Self {
        Self {
            router: Arc::default(),
            server_state: Arc::default(),
            quit_tx: Arc::new(quit_tx),
        }
    }
}

impl HttpServerProvider {
    /// Start or restart the HTTP server with new settings
    async fn start_server(
        &self,
        settings: Arc<ServiceSettings>,
        header: String,
    ) -> anyhow::Result<Result<(), String>> {
        // Shutdown previous server if running
        {
            let mut server_state = self.server_state.write().await;
            if let Some(state) = server_state.take() {
                info!("Shutting down previous HTTP server");
                state.shutdown();
            }
        }

        // Update the header in the router
        {
            let mut router = self.router.write().await;
            router.header = header;
        }

        let addr = settings.address;
        info!(
            %addr,
            "httpserver starting listener in host-based mode",
        );

        let cors = match get_cors_layer(&settings) {
            Result::Ok(cors) => cors,
            Result::Err(e) => return Ok(Err(format!("failed to configure CORS: {e}"))),
        };
        let listener = match get_tcp_listener(&settings) {
            Result::Ok(listener) => listener,
            Result::Err(e) => return Ok(Err(format!("failed to bind TCP listener: {e}"))),
        };
        let service = handle_request.layer(cors);

        let handle = axum_server::Handle::new();
        let task_handle = handle.clone();
        let task_router = Arc::clone(&self.router);

        let task = if let (Some(crt), Some(key)) =
            (&settings.tls_cert_file, &settings.tls_priv_key_file)
        {
            debug!(?addr, "bind HTTPS listener");
            let tls = match RustlsConfig::from_pem_file(crt, key).await {
                Result::Ok(tls) => tls,
                Result::Err(e) => return Ok(Err(format!("failed to construct TLS config: {e}"))),
            };

            tokio::spawn(async move {
                if let Err(e) = axum_server::from_tcp_rustls(listener, tls)
                    .handle(task_handle)
                    .serve(
                        service
                            .with_state(RequestContext {
                                router: task_router,
                                scheme: http::uri::Scheme::HTTPS,
                                settings: Arc::clone(&settings),
                            })
                            .into_make_service(),
                    )
                    .await
                {
                    error!(error = %e, "failed to serve HTTPS for host-based mode");
                }
            })
        } else {
            debug!(?addr, "bind HTTP listener");

            tokio::spawn(async move {
                if let Err(e) = axum_server::from_tcp(listener)
                    .handle(task_handle)
                    .serve(
                        service
                            .with_state(RequestContext {
                                router: task_router,
                                scheme: http::uri::Scheme::HTTP,
                                settings: Arc::clone(&settings),
                            })
                            .into_make_service(),
                    )
                    .await
                {
                    error!(error = %e, "failed to serve HTTP for host-based mode");
                }
            })
        };

        // Store the new server state
        {
            let mut server_state = self.server_state.write().await;
            *server_state = Some(ServerState { handle, task });
        }

        Ok(Ok(()))
    }
}

impl configurable::Handler<Option<Context>> for HttpServerProvider {
    #[instrument(level = "debug", skip_all)]
    async fn update_base_config(
        &self,
        _cx: Option<Context>,
        config: wasmcloud_provider_sdk::types::BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config: HashMap<String, String> = config.config.into_iter().collect();

        let default_address = match config
            .get("default_address")
            .map(|s| SocketAddr::from_str(s))
            .transpose()
        {
            Result::Ok(addr) => addr,
            Result::Err(e) => return Ok(Err(format!("failed to parse default_address: {e}"))),
        };

        let header = config
            .get("header")
            .map(String::as_str)
            .unwrap_or("host")
            .to_lowercase();

        let settings = match load_settings(default_address, &config) {
            Result::Ok(settings) => Arc::new(settings),
            Result::Err(e) => return Ok(Err(format!("failed to load settings in host mode: {e}"))),
        };

        self.start_server(settings, header).await
    }

    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn update_interface_export_config(
        &self,
        _cx: Option<Context>,
        _source_id: String,
        _link_name: String,
        _link_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(target_id))]
    async fn update_interface_import_config(
        &self,
        _cx: Option<Context>,
        target_id: String,
        link_name: String,
        interface_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config: HashMap<String, String> = interface_config.config.into_iter().collect();

        let Some(host) = config.get("host") else {
            error!(
                ?config,
                ?target_id,
                "host not found in link config, cannot register host"
            );
            return Ok(Err(format!(
                "host not found in link config, cannot register host for component {}",
                target_id
            )));
        };

        let target = Arc::from(target_id.as_str());
        let name = Arc::from(link_name.as_str());

        let key = (Arc::clone(&target), Arc::clone(&name));

        let mut router = self.router.write().await;
        if router.components.contains_key(&key) {
            return Ok(Err(format!(
                "Component {target} already has a host registered with link name {name}"
            )));
        }
        if router.hosts.contains_key(host.as_str()) {
            return Ok(Err(format!(
                "Host {host} already in use by a different component"
            )));
        }

        let wrpc = match get_connection().get_wrpc_client(&target).await {
            Result::Ok(wrpc) => wrpc,
            Result::Err(e) => {
                return Ok(Err(format!("failed to construct wRPC client: {e}")));
            }
        };

        let host = Arc::from(host.clone());
        // Insert the host into the hosts map for future lookups
        router.components.insert(key, Arc::clone(&host));
        router.hosts.insert(host, (target, wrpc));
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

        let mut router = self.router.write().await;
        let host = router
            .components
            .remove(&(Arc::from(target_id), Arc::from(link_name)));
        if let Some(host) = host {
            router.hosts.remove(&host);
        }
        Ok(Ok(()))
    }

    #[instrument(level = "info", skip_all, fields(source_id))]
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
        let mut server_state = self.server_state.write().await;
        if let Some(state) = server_state.take() {
            state.shutdown();
        }

        // Signal the provider to shut down
        let _ = self.quit_tx.send(());
        Ok(Ok(()))
    }
}

#[derive(Clone)]
struct RequestContext {
    router: Arc<RwLock<Router>>,
    scheme: http::uri::Scheme,
    settings: Arc<ServiceSettings>,
}

/// Handle an HTTP request by looking up the component ID for the host and invoking the component
#[instrument(level = "debug", skip(router, settings))]
async fn handle_request(
    extract::State(RequestContext {
        router,
        scheme,
        settings,
    }): extract::State<RequestContext>,
    axum_extra::extract::Host(authority): axum_extra::extract::Host,
    request: extract::Request,
) -> impl axum::response::IntoResponse {
    let timeout = settings.timeout_ms.map(Duration::from_millis);
    let req = build_request(request, scheme, authority, &settings).map_err(|err| *err)?;

    let Some(host_header) = req.headers().get(router.read().await.header.as_str()) else {
        Err((http::StatusCode::BAD_REQUEST, "missing host header"))?
    };

    let lookup_host = host_header
        .to_str()
        .map_err(|_| (http::StatusCode::BAD_REQUEST, "invalid host header"))?;

    let Some((target_component, wrpc)) = router.read().await.hosts.get(lookup_host).cloned() else {
        Err((http::StatusCode::NOT_FOUND, "host not found"))?
    };

    axum::response::Result::<_, axum::response::ErrorResponse>::Ok(
        invoke_component(
            &wrpc,
            &target_component,
            req,
            timeout,
            settings.cache_control.as_ref(),
        )
        .await,
    )
}
