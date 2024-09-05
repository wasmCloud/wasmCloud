//! This module contains the implementation of the `wrpc:http/incoming-handler` provider in path-based mode.
//!
//! In path-based mode, the HTTP server listens on a single address and routes requests to different components
//! based on the path of the request.

use core::time::Duration;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use axum::extract::{self};
use axum::handler::Handler;
use axum_server::tls_rustls::RustlsConfig;
use axum_server::Handle;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, instrument};
use wasmcloud_provider_sdk::{HostData, LinkConfig, LinkDeleteInfo, Provider};

use crate::{
    build_request, get_cors_layer, get_tcp_listener, invoke_component, load_settings,
    ServiceSettings,
};

/// This struct holds both the forward and reverse mappings for path-based routing
/// so that they can be modified by just acquiring a single lock in the [`HttpServerProvider`]
#[derive(Debug, Default)]
struct Router {
    /// Lookup from a path to the component ID that is handling that path
    paths: HashMap<String, String>,
    /// Reverse lookup to find the path for a (component,link_name) pair
    components: HashMap<(String, String), String>,
}

/// `wrpc:http/incoming-handler` provider implementation with path-based routing
#[derive(Clone)]
pub struct HttpServerProvider {
    /// Struct that holds the routing information based on path/component_id
    path_router: Arc<RwLock<Router>>,
    /// [`Handle`] to the server task
    handle: Handle,
    /// Task handle for the server task
    task: Arc<JoinHandle<()>>,
}

impl Drop for HttpServerProvider {
    fn drop(&mut self) {
        self.handle.shutdown();
        self.task.abort();
    }
}

impl HttpServerProvider {
    pub(crate) async fn new(host_data: &HostData) -> anyhow::Result<Self> {
        let default_address = host_data
            .config
            .get("default_address")
            .map(|s| SocketAddr::from_str(s))
            .transpose()
            .context("failed to parse default_address")?;
        let settings = Arc::new(
            load_settings(default_address, &host_data.config)
                .context("failed to load settings in path mode")?,
        );

        let path_router = Arc::new(RwLock::new(Router::default()));

        let addr = settings.address;
        info!(
            %addr,
            "httpserver starting listener in path-based mode",
        );
        let cors = get_cors_layer(settings.clone())?;
        let listener = get_tcp_listener(settings.clone())?;
        let service = handle_request.layer(cors);

        let handle = axum_server::Handle::new();
        let task_handle = handle.clone();
        let task_router = path_router.clone();
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
                                router: task_router,
                                scheme: http::uri::Scheme::HTTPS,
                                settings: settings.clone(),
                            })
                            .into_make_service(),
                    )
                    .await
                {
                    error!(error = %e, "failed to serve HTTPS for path-based mode");
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
                                settings: settings.clone(),
                            })
                            .into_make_service(),
                    )
                    .await
                {
                    error!(error = %e, "failed to serve HTTP for path-based mode");
                }
            })
        };

        Ok(Self {
            path_router,
            handle,
            task: Arc::new(task),
        })
    }
}

impl Provider for HttpServerProvider {
    /// This is called when the HTTP server provider is linked to a component
    ///
    /// This HTTP server mode will register the path in the link for routing to the target
    /// component when a request is received on the listen address.
    async fn receive_link_config_as_source(
        &self,
        link_config: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let Some(path) = link_config.config.get("path") else {
            error!(?link_config.config, ?link_config.target_id, "path not found in link config, cannot register path");
            bail!(
                "path not found in link config, cannot register path for component {}",
                link_config.target_id
            );
        };

        let mut path_router = self.path_router.write().await;
        if path_router.components.contains_key(&(
            link_config.target_id.to_string(),
            link_config.link_name.to_string(),
        )) {
            // When we can return errors from links, tell the host this was invalid
            bail!(
                "Component {} already has a path registered with link name {}",
                link_config.target_id,
                link_config.link_name
            );
        }
        if path_router.paths.contains_key(path.as_str()) {
            // When we can return errors from links, tell the host this was invalid
            bail!("Path {path} already in use by a different component");
        }

        // Insert the path into the paths map for future lookups
        path_router.components.insert(
            (
                link_config.target_id.to_string(),
                link_config.link_name.to_string(),
            ),
            path.to_string(),
        );
        path_router
            .paths
            .insert(path.to_string(), link_config.target_id.to_string());

        Ok(())
    }

    /// Remove the path for a particular component/link_name pair
    #[instrument(level = "info", skip_all, fields(target_id = info.get_target_id()))]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_target_id();
        let link_name = info.get_link_name();

        let mut path_router = self.path_router.write().await;
        let path = path_router
            .components
            .remove(&(component_id.to_string(), link_name.to_string()));
        if let Some(path) = path {
            path_router.paths.remove(&path);
        }

        Ok(())
    }

    /// Handle shutdown request by shutting down the http server task
    async fn shutdown(&self) -> anyhow::Result<()> {
        self.handle.shutdown();
        self.task.abort();

        Ok(())
    }
}

#[derive(Clone, Debug)]
struct RequestContext {
    router: Arc<RwLock<Router>>,
    scheme: http::uri::Scheme,
    settings: Arc<ServiceSettings>,
}

/// Handle an HTTP request by looking up the component ID for the path and invoking the component
#[instrument(level = "debug", skip(router, settings))]
async fn handle_request(
    extract::State(RequestContext {
        router,
        scheme,
        settings,
    }): extract::State<RequestContext>,
    extract::Host(authority): extract::Host,
    request: extract::Request,
) -> impl axum::response::IntoResponse {
    let timeout = settings.timeout_ms.map(Duration::from_millis);
    let req = build_request(request, scheme, authority, settings.clone())?;
    let path = req.uri().path();
    let Some(target_component) = router.read().await.paths.get(path).cloned() else {
        return Err((http::StatusCode::NOT_FOUND, "path not found".to_string()));
    };
    Ok(invoke_component(
        target_component,
        req,
        timeout,
        settings.cache_control.as_ref(),
    )
    .await)
}
