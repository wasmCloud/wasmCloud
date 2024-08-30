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
use wasmcloud_provider_sdk::{HostData, LinkConfig, LinkDeleteInfo, Provider};

use crate::settings::default_listen_address;
use crate::{
    build_request, get_cors_layer, get_tcp_listener, invoke_component, load_settings,
    ServiceSettings,
};

/// `wrpc:http/incoming-handler` provider implementation in address mode
#[derive(Clone)]
pub struct HttpServerProvider {
    default_address: SocketAddr,
    // Map from (component_id, link_name) to HttpServerCore
    /// Stores http_server handlers for each linked component
    component_handlers: Arc<RwLock<HashMap<(String, String), HttpServerCore>>>,
}

impl Default for HttpServerProvider {
    fn default() -> Self {
        Self {
            default_address: default_listen_address(),
            component_handlers: Default::default(),
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
            component_handlers: Default::default(),
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

        // Start a server instance that calls the given component
        let http_server = HttpServerCore::new(Arc::new(settings), link_config.target_id)
            .await
            .context("httpserver failed to start listener for component")?;

        // Save the component and server instance locally
        self.component_handlers.write().await.insert(
            (
                link_config.target_id.to_string(),
                link_config.link_name.to_string(),
            ),
            http_server,
        );
        Ok(())
    }

    /// Handle notification that a link is dropped - stop the http listener
    #[instrument(level = "info", skip_all, fields(target_id = info.get_target_id()))]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_target_id();
        let link_name = info.get_link_name();
        if let Some(server) = self
            .component_handlers
            .write()
            .await
            .remove(&(component_id.to_string(), link_name.to_string()))
        {
            info!(
                component_id,
                link_name, "httpserver stopping listener for component"
            );
            server.handle.shutdown();
        }
        Ok(())
    }

    /// Handle shutdown request by shutting down all the http server threads
    async fn shutdown(&self) -> anyhow::Result<()> {
        // empty the component link data and stop all servers
        self.component_handlers.write().await.clear();
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct RequestContext {
    target: String,
    settings: Arc<ServiceSettings>,
    scheme: http::uri::Scheme,
}

/// Handle an HTTP request by invoking the target component as configured in the listener
#[instrument(level = "debug", skip(settings))]
async fn handle_request(
    extract::State(RequestContext {
        target,
        settings,
        scheme,
    }): extract::State<RequestContext>,
    extract::Host(authority): extract::Host,
    request: extract::Request,
) -> impl axum::response::IntoResponse {
    let timeout = settings.timeout_ms.map(Duration::from_millis);
    let req = build_request(request, scheme, authority, settings.clone())?;
    Ok::<_, (http::StatusCode, String)>(
        invoke_component(target, req, timeout, settings.cache_control.as_ref()).await,
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
    #[instrument]
    pub async fn new(settings: Arc<ServiceSettings>, target: &str) -> anyhow::Result<Self> {
        let addr = settings.address;
        info!(
            %addr,
            component_id = target,
            "httpserver starting listener for target",
        );
        let cors = get_cors_layer(settings.clone())?;
        let service = handle_request.layer(cors);
        let handle = axum_server::Handle::new();
        let listener = get_tcp_listener(settings.clone())?;

        let target = target.to_owned();
        let task_handle = handle.clone();
        let task = if let (Some(crt), Some(key)) =
            (&settings.tls_cert_file, &settings.tls_priv_key_file)
        {
            debug!(?addr, "bind HTTPS listener");
            let tls = RustlsConfig::from_pem_file(crt, key)
                .await
                .context("failed to construct TLS config")?;

            tokio::spawn(async move {
                if let Err(e) = axum_server::from_tcp_rustls(listener, tls)
                    .handle(task_handle)
                    .serve(
                        service
                            .with_state(RequestContext {
                                target: target.clone(),
                                settings,
                                scheme: http::uri::Scheme::HTTPS,
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

            tokio::spawn(async move {
                if let Err(e) = axum_server::from_tcp(listener)
                    .handle(task_handle)
                    .serve(
                        service
                            .with_state(RequestContext {
                                target: target.clone(),
                                settings,
                                scheme: http::uri::Scheme::HTTP,
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
