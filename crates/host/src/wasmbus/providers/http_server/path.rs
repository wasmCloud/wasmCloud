use core::net::SocketAddr;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use http::header::HOST;
use http::uri::Scheme;
use http::Uri;
use http_body_util::BodyExt as _;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, error, info_span, instrument, trace_span, warn, Instrument as _, Span};
use wasmcloud_provider_sdk::{LinkConfig, LinkDeleteInfo};
use wasmcloud_tracing::KeyValue;
use wrpc_interface_http::ServeIncomingHandlerWasmtime as _;

use crate::wasmbus::{Component, InvocationContext};

use super::listen;

/// This struct holds both the forward and reverse mappings for path-based routing
/// so that they can be modified by just acquiring a single lock in the [`HttpServerProvider`]
#[derive(Default)]
pub(crate) struct Router {
    /// Lookup from a path to the component ID that is handling that path
    pub(crate) paths: HashMap<Arc<str>, Arc<str>>,
    /// Reverse lookup to find the path for a (component,link_name) pair
    pub(crate) components: HashMap<(Arc<str>, Arc<str>), Arc<str>>,
}

pub(crate) struct Provider {
    /// Handle to the server task
    pub(crate) handle: JoinHandle<()>,
    /// Struct that holds the routing information based on path/component_id
    pub(crate) path_router: Arc<RwLock<Router>>,
}

impl wasmcloud_provider_sdk::Provider for Provider {
    #[instrument(level = "debug", skip_all)]
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

        let target = Arc::from(link_config.target_id);
        let name = Arc::from(link_config.link_name);
        let key = (Arc::clone(&target), Arc::clone(&name));

        let mut path_router = self.path_router.write().await;
        if path_router.components.contains_key(&key) {
            bail!("Component {target} already has a path registered with link name {name}");
        }
        if path_router.paths.contains_key(path.as_str()) {
            bail!("Path {path} already in use by a different component");
        }

        let path = Arc::from(path.clone());
        // Insert the path into the paths map for future lookups
        path_router.components.insert(key, Arc::clone(&path));
        path_router.paths.insert(path, target);

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        debug!(
            source = info.get_source_id(),
            target = info.get_target_id(),
            link = info.get_link_name(),
            "deleting http path link"
        );
        let component_id = info.get_target_id();
        let link_name = info.get_link_name();

        let mut path_router = self.path_router.write().await;
        let path = path_router
            .components
            .remove(&(Arc::from(component_id), Arc::from(link_name)));
        if let Some(path) = path {
            path_router.paths.remove(&path);
        }

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn shutdown(&self) -> anyhow::Result<()> {
        self.handle.abort();
        Ok(())
    }
}

impl Provider {
    pub(crate) async fn new(
        address: SocketAddr,
        components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
        lattice_id: Arc<str>,
        host_id: Arc<str>,
    ) -> anyhow::Result<Self> {
        let path_router: Arc<RwLock<Router>> = Arc::default();

        let lattice_id_fn = Arc::clone(&lattice_id);
        let host_id_fn = Arc::clone(&host_id);
        let components_fn = Arc::clone(&components);
        let path_router_fn = Arc::clone(&path_router);
        let handle = listen(
            address,
            move |req: hyper::Request<hyper::body::Incoming>| {
                let lattice_id = Arc::clone(&lattice_id_fn);
                let host_id = Arc::clone(&host_id_fn);
                let components = Arc::clone(&components_fn);
                let path_router = Arc::clone(&path_router_fn);
                async move {
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

                    // TODO(#3705): Propagate trace context from headers
                    let mut uri = Uri::builder().scheme(scheme.unwrap_or(Scheme::HTTP));
                    let component = if let Some(path_and_query) = path_and_query {
                        let component_id = {
                            let router = path_router.read().await;
                            let Some(component_id) = router.paths.get(path_and_query.path()) else {
                                warn!(path = path_and_query.path(), "received request for unregistered http path");
                                return anyhow::Ok(
                                    http::Response::builder()
                                        .status(404)
                                        .body(wasmtime_wasi_http::body::HyperOutgoingBody::default())
                                        .context("failed to construct missing path error response")?,
                                );
                            };
                            component_id.to_string()
                        };

                        uri = uri.path_and_query(path_and_query);

                        let components = components.read().await;
                        let component = components
                            .get(&component_id)
                            .context("linked component not found")?;
                        Arc::clone(component)
                    } else {
                        warn!("path not found in URI, could not look up component");
                        return anyhow::Ok(
                            http::Response::builder()
                                .status(404)
                                .body(wasmtime_wasi_http::body::HyperOutgoingBody::default())
                                .context("failed to construct missing path error response")?,
                        );
                    };
                    if let Some(authority) = authority {
                        uri = uri.authority(authority);
                    } else if let Some(authority) = headers.get("X-Forwarded-Host") {
                        uri = uri.authority(authority.as_bytes());
                    } else if let Some(authority) = headers.get(HOST) {
                        uri = uri.authority(authority.as_bytes());
                    }

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
                        .instrument(trace_span!("acquire_permit"))
                        .await
                        .context("failed to acquire execution permit")?;
                    let res = component
                        .instantiate(component.handler.copy_for_new(), component.events.clone())
                        .handle(
                            InvocationContext {
                                span: Span::current(),
                                start_at: Instant::now(),
                                attributes: vec![
                                    KeyValue::new(
                                        "component.ref",
                                        Arc::clone(&component.image_reference),
                                    ),
                                    KeyValue::new("lattice", Arc::clone(&lattice_id)),
                                    KeyValue::new("host", Arc::clone(&host_id)),
                                ],
                            },
                            req,
                        )
                        .await?;
                    let res = res?;
                    anyhow::Ok(res)
                }
                .instrument(info_span!("handle"))
            },
        )
        .await
        .context("failed to listen on address for path based http server")?;

        Ok(Provider {
            handle,
            path_router,
        })
    }
}
