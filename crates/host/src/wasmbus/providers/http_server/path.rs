use core::net::SocketAddr;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _, Ok};
use http::header::HOST;
use http::uri::Scheme;
use http::Uri;
use http_body_util::BodyExt as _;
use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::{debug, error, info_span, instrument, trace_span, warn, Instrument as _, Span};
use wasmcloud_tracing::KeyValue;
use wrpc_interface_http::ServeIncomingHandlerWasmtime as _;

use crate::bindings::exports::wrpc::extension;
use crate::bindings::wrpc::extension::types::{
    BaseConfig, BindRequest, BindResponse, HealthCheckResponse, InterfaceConfig,
};
use crate::wasmbus::{Component, InvocationContext};

use super::listen;

/// This struct holds both the forward and reverse mappings for path-based routing
/// so that they can be modified by just acquiring a single lock in the [`HttpServerProvider`]
#[derive(Default, Clone)]
pub(crate) struct Router {
    /// Lookup from a path to the component ID that is handling that path
    pub(crate) paths: HashMap<Arc<str>, Arc<str>>,
    /// Reverse lookup to find the path for a (component,link_name) pair
    pub(crate) components: HashMap<(Arc<str>, Arc<str>), Arc<str>>,
}

#[derive(Clone)]
pub(crate) struct Provider {
    /// Handle to the server task. The use of the [`JoinSet`] allows for the server to be
    /// gracefully shutdown when the provider is shutdown
    #[allow(unused)]
    pub(crate) handle: Arc<tokio::sync::Mutex<JoinSet<()>>>,
    /// Struct that holds the routing information based on path/component_id
    pub(crate) path_router: Arc<RwLock<Router>>,
    /// Broadcast sender for shutdown signaling
    pub(crate) quit_tx: broadcast::Sender<()>,
}

impl extension::manageable::Handler<Option<wasmcloud_provider_sdk::Context>> for Provider {
    async fn bind(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        anyhow::Ok(Result::<BindResponse, String>::Ok(BindResponse {
            identity_token: None,
            provider_pubkey: None,
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        anyhow::Ok(Result::<HealthCheckResponse, String>::Ok(
            HealthCheckResponse {
                healthy: true,
                message: Some("OK".to_string()),
            },
        ))
    }

    /// Handle shutdown request by signaling the provider to shut down
    async fn shutdown(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
    ) -> anyhow::Result<Result<(), String>> {
        // NOTE: The result is ignored because the channel will be closed if the last
        // receiver is dropped, which is a valid way to shut down.
        let _ = self.quit_tx.send(());
        anyhow::Ok(Result::<(), String>::Ok(()))
    }
}

impl extension::configurable::Handler<Option<wasmcloud_provider_sdk::Context>> for Provider {
    #[instrument(level = "debug", skip_all)]
    async fn update_base_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _config: BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        anyhow::Ok(Result::<(), String>::Ok(()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn update_interface_export_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _source_id: String,
        _link_name: String,
        _config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        anyhow::Ok(Result::<(), String>::Ok(()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn update_interface_import_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        target_id: String,
        link_name: String,
        config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config_map: HashMap<String, String> = config.config.into_iter().collect();
        match self.put_link(&target_id, &link_name, &config_map).await {
            Result::Ok(()) => anyhow::Ok(Result::<(), String>::Ok(())),
            Result::Err(e) => anyhow::Ok(Result::<(), String>::Err(format!(
                "Failed to register path for target {} link {}: {}",
                target_id, link_name, e
            ))),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_interface_import_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        target_id: String,
        link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        match self.delete_link(&target_id, &link_name).await {
            Result::Ok(()) => anyhow::Ok(Result::<(), String>::Ok(())),
            Result::Err(e) => anyhow::Ok(Result::<(), String>::Err(format!(
                "Failed to delete path for target {} link {}: {}",
                target_id, link_name, e
            ))),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_interface_export_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _source_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        anyhow::Ok(Result::<(), String>::Ok(()))
    }
}

impl Provider {
    #[instrument(level = "debug", skip(self))]
    async fn put_link(
        &self,
        component_id: &str,
        link_name: &str,
        config: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let Some(path) = config.get("path") else {
            error!(
                ?config,
                %component_id,
                %link_name,
                "path not found in link config, cannot register path"
            );
            bail!(
                "path not found in link config for component {} link {}",
                component_id,
                link_name
            );
        };

        let component_id_arc: Arc<str> = Arc::from(component_id);
        let link_name_arc: Arc<str> = Arc::from(link_name);
        let path_arc: Arc<str> = Arc::from(path.as_str());
        let link_key = (component_id_arc.clone(), link_name_arc);

        let mut path_router = self.path_router.write().await;

        // Check if this link already has a path registered
        if path_router.components.contains_key(&link_key) {
            bail!(
                "Component {} link {} already has a path registered",
                component_id,
                link_name
            );
        }

        // Check if this path is already in use by a different component
        if path_router.paths.contains_key(&path_arc) {
            bail!("Path {} already in use by a different component", path);
        }

        // Store the mappings: (component_id, link_name) -> path, path -> component_id
        path_router.components.insert(link_key, path_arc.clone());
        path_router.paths.insert(path_arc, component_id_arc);

        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    async fn delete_link(&self, component_id: &str, link_name: &str) -> anyhow::Result<()> {
        debug!(%component_id, %link_name, "deleting http path link");

        let link_key = (Arc::from(component_id), Arc::from(link_name));
        let mut path_router = self.path_router.write().await;
        if let Some(path) = path_router.components.remove(&link_key) {
            // Remove the reverse mapping
            path_router.paths.remove(&path);
        }

        Ok(())
    }
}

impl Provider {
    pub(crate) async fn new(
        address: SocketAddr,
        components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
        lattice_id: Arc<str>,
        host_id: Arc<str>,
        quit_tx: broadcast::Sender<()>,
    ) -> anyhow::Result<Self> {
        let path_router: Arc<RwLock<Router>> = Arc::default();
        let handle = listen(address, {
            let path_router = Arc::clone(&path_router);
            move |req: hyper::Request<hyper::body::Incoming>| {
                let lattice_id = Arc::clone(&lattice_id);
                let host_id = Arc::clone(&host_id);
                let components = Arc::clone(&components);
                let path_router = Arc::clone(&path_router);
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
            }
        })
        .await
        .context("failed to listen on address for path based http server")?;

        Ok(Provider {
            handle: Arc::new(tokio::sync::Mutex::new(handle)),
            path_router,
            quit_tx,
        })
    }
}

#[cfg(test)]
mod test {
    use std::{collections::HashMap, sync::Arc};

    use anyhow::Context as _;
    use tokio::task::JoinSet;

    /// Ensure we can register and deregister a bunch of paths properly
    #[tokio::test]
    async fn can_manage_paths() -> anyhow::Result<()> {
        let (quit_tx, _quit_rx) = tokio::sync::broadcast::channel(1);
        let provider = super::Provider {
            handle: Arc::new(tokio::sync::Mutex::new(JoinSet::new())),
            path_router: Arc::default(),
            quit_tx,
        };

        // Put path registrations:
        // /foo -> foo
        // /api/bar -> bar
        // /foo/api/baz -> baz
        provider
            .put_link(
                "foo",
                "default",
                &HashMap::from([("path".to_string(), "/foo".to_string())]),
            )
            .await
            .context("should register foo path")?;
        provider
            .put_link(
                "bar",
                "default",
                &HashMap::from([("path".to_string(), "/api/bar".to_string())]),
            )
            .await
            .context("should register bar path")?;
        provider
            .put_link(
                "baz",
                "default",
                &HashMap::from([("path".to_string(), "/foo/api/baz".to_string())]),
            )
            .await
            .context("should register baz path")?;

        {
            let router = provider.path_router.read().await;
            assert_eq!(router.paths.len(), 3);
            assert_eq!(router.components.len(), 3);
            assert!(router
                .paths
                .get("/foo")
                .is_some_and(|target| &target.to_string() == "foo"));
            assert!(router
                .components
                .get(&(Arc::from("foo"), Arc::from("default")))
                .is_some_and(|p| &p.to_string() == "/foo"));
            assert!(router
                .paths
                .get("/api/bar")
                .is_some_and(|target| &target.to_string() == "bar"));
            assert!(router
                .components
                .get(&(Arc::from("bar"), Arc::from("default")))
                .is_some_and(|p| &p.to_string() == "/api/bar"));
            assert!(router
                .paths
                .get("/foo/api/baz")
                .is_some_and(|target| &target.to_string() == "baz"));
            assert!(router
                .components
                .get(&(Arc::from("baz"), Arc::from("default")))
                .is_some_and(|p| &p.to_string() == "/foo/api/baz"));
        }

        // Rejecting reserved paths / linked components
        assert!(
            provider
                .put_link(
                    "notbaz",
                    "default",
                    &HashMap::from([("path".to_string(), "/foo/api/baz".to_string())]),
                )
                .await
                .is_err(),
            "should fail to register a path that's already registered"
        );
        assert!(
            provider
                .put_link(
                    "baz",
                    "default",
                    &HashMap::from([("path".to_string(), "/foo/api/notbaz".to_string())]),
                )
                .await
                .is_err(),
            "should fail to register a path to a component that already has a path"
        );

        // Delete path registrations
        provider
            .delete_link("builtin", "foo", "default")
            .await
            .context("should delete link")?;
        provider
            .delete_link("builtin", "bar", "default")
            .await
            .context("should delete link")?;
        provider
            .delete_link("builtin", "baz", "default")
            .await
            .context("should delete link")?;
        {
            let router = provider.path_router.read().await;
            assert!(router.paths.is_empty());
            assert!(router.components.is_empty());
        }

        Ok(())
    }
}
