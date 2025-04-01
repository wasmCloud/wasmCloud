use core::net::SocketAddr;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use http::header::HOST;
use http::uri::Scheme;
use http::Uri;
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt as _;
use tokio::time::Instant;
use tokio::{sync::RwLock, task::JoinSet};
use tracing::{debug, error, info_span, instrument, trace_span, warn, Instrument as _, Span};
use wasmcloud_provider_sdk::{LinkConfig, LinkDeleteInfo};
use wasmcloud_tracing::KeyValue;
use wasmtime_wasi_http::bindings::http::types::ErrorCode;
use wrpc_interface_http::ServeIncomingHandlerWasmtime as _;

use crate::wasmbus::{Component, InvocationContext};

use super::listen;

/// This struct holds both the forward and reverse mappings for host-based routing
/// so that they can be modified by just acquiring a single lock in the [`HttpServerProvider`]
#[derive(Default)]
pub(crate) struct Router {
    /// Lookup from a host to the component ID that is handling that host
    hosts: HashMap<Arc<str>, Arc<str>>,
    /// Reverse lookup to find the host for a (component,link_name) pair
    components: HashMap<(Arc<str>, Arc<str>), Arc<str>>,
    /// Header to match for host-based routing
    header: String,
}

pub(crate) struct Provider {
    /// Handle to the server task. The use of the [`JoinSet`] allows for the server to be
    /// gracefully shutdown when the provider is shutdown
    #[allow(unused)]
    pub(crate) handle: JoinSet<()>,
    /// Struct that holds the routing information based on host/component_id
    pub(crate) host_router: Arc<RwLock<Router>>,
}

// Implementations of put and delete link are done in the `impl Provider` block to aid in testing
impl wasmcloud_provider_sdk::Provider for Provider {
    #[instrument(level = "debug", skip_all)]
    async fn receive_link_config_as_source(&self, link: LinkConfig<'_>) -> Result<()> {
        self.put_link(link.target_id, link.link_name, link.config)
            .await
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> Result<()> {
        self.delete_link(
            info.get_source_id(),
            info.get_target_id(),
            info.get_link_name(),
        )
        .await
    }
}

impl Provider {
    #[instrument(level = "debug", skip(self))]
    async fn put_link(
        &self,
        target_id: &str,
        link_name: &str,
        config: &HashMap<String, String>,
    ) -> Result<()> {
        let Some(host) = config.get("host") else {
            error!(
                ?config,
                ?target_id,
                "host not found in link config, cannot register host"
            );
            bail!("host not found in link config, cannot register host for component {target_id}");
        };

        let target = Arc::from(target_id);
        let name = Arc::from(link_name);
        let key = (Arc::clone(&target), Arc::clone(&name));

        let mut router = self.host_router.write().await;
        if router.components.contains_key(&key) {
            // Ensure the current host doesn't differ for the given component
            if router
                .components
                .get(&key)
                .map(|val| **val != *host)
                .unwrap_or(false)
            {
                // When we can return errors from links, tell the host this was invalid
                bail!("Component {target_id} already has a host registered with link name {name}");
            }
        }
        if router.hosts.contains_key(host.as_str()) {
            // Ensure the current component doesn't differ for the given host
            if router
                .hosts
                .get(host.as_str())
                .map(|val| *val != target)
                .unwrap_or(false)
            {
                // When we can return errors from links, tell the host this was invalid
                bail!("Host {host} already in use by a different component");
            }
        }

        let host = Arc::from(host.clone());
        // Insert the host into the hosts map for future lookups
        router.components.insert(key, Arc::clone(&host));
        router.hosts.insert(host, target);

        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    async fn delete_link(&self, source_id: &str, target_id: &str, link_name: &str) -> Result<()> {
        debug!(
            source = source_id,
            target = target_id,
            link = link_name,
            "deleting http host link"
        );

        let mut router = self.host_router.write().await;
        let host = router
            .components
            .remove(&(Arc::from(target_id), Arc::from(link_name)));
        if let Some(host) = host {
            router.hosts.remove(&host);
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
        host_header: Option<String>,
    ) -> Result<Self> {
        let host_router = Arc::new(RwLock::new(Router {
            hosts: HashMap::new(),
            components: HashMap::new(),
            header: host_header.unwrap_or_else(|| HOST.to_string()),
        }));
        let handle = listen(address, {
            let host_router = Arc::clone(&host_router);
            move |req: hyper::Request<hyper::body::Incoming>| {
                let lattice_id = Arc::clone(&lattice_id);
                let host_id = Arc::clone(&host_id);
                let components = Arc::clone(&components);
                let host_router = Arc::clone(&host_router);
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

                    let Some(host_header) = headers.get(host_router.read().await.header.as_str())
                    else {
                        warn!("received request with no host header");
                        return build_bad_request_error("missing host header");
                    };

                    let Ok(lookup_host) = host_header.to_str() else {
                        warn!("received request with invalid host header");
                        return build_bad_request_error("invalid host header");
                    };

                    // TODO(#3705): Propagate trace context from headers
                    let mut uri = Uri::builder().scheme(scheme.unwrap_or(Scheme::HTTP));
                    let component = {
                        let component_id = {
                            let router = host_router.read().await;
                            let Some(component_id) = router.hosts.get(lookup_host) else {
                                warn!(host = lookup_host, "received request for unregistered host");
                                return http::Response::builder()
                                    .status(404)
                                    .body(wasmtime_wasi_http::body::HyperOutgoingBody::new(
                                        BoxBody::new(
                                            http_body_util::Empty::new()
                                                .map_err(|_| ErrorCode::InternalError(None)),
                                        ),
                                    ))
                                    .context("failed to construct missing host error response");
                            };
                            component_id.to_string()
                        };

                        let components = components.read().await;
                        let component = components
                            .get(&component_id)
                            .context("linked component not found")?;
                        Arc::clone(component)
                    };

                    if let Some(path_and_query) = path_and_query {
                        uri = uri.path_and_query(path_and_query);
                    }

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
                    Ok(res)
                }
                .instrument(info_span!("handle"))
            }
        })
        .await
        .context("failed to listen on address for host based http server")?;

        Ok(Provider {
            handle,
            host_router,
        })
    }
}

/// Build a bad request error
fn build_bad_request_error(
    message: &str,
) -> Result<http::Response<wasmtime_wasi_http::body::HyperOutgoingBody>> {
    http::Response::builder()
        .status(http::StatusCode::BAD_REQUEST)
        .body(wasmtime_wasi_http::body::HyperOutgoingBody::new(
            BoxBody::new(
                http_body_util::Full::new(bytes::Bytes::copy_from_slice(message.as_bytes()))
                    .map_err(|_| ErrorCode::InternalError(None)),
            ),
        ))
        .with_context(|| format!("failed to construct host error response: {message}"))
}

#[cfg(test)]
mod test {
    use std::{collections::HashMap, sync::Arc};

    use anyhow::Context as _;
    use tokio::task::JoinSet;

    /// Ensure we can register and deregister a bunch of hosts properly
    #[tokio::test]
    async fn can_manage_hosts() -> anyhow::Result<()> {
        let provider = super::Provider {
            handle: JoinSet::new(),
            host_router: Arc::default(),
        };

        // Put host registrations:
        // foo.com -> foo
        // bar.com -> bar
        // baz.com -> baz
        provider
            .put_link(
                "foo",
                "default",
                &HashMap::from([("host".to_string(), "foo.com".to_string())]),
            )
            .await
            .context("should register foo host")?;
        provider
            .put_link(
                "bar",
                "default",
                &HashMap::from([("host".to_string(), "bar.com".to_string())]),
            )
            .await
            .context("should register bar host")?;
        provider
            .put_link(
                "baz",
                "default",
                &HashMap::from([("host".to_string(), "baz.com".to_string())]),
            )
            .await
            .context("should register baz host")?;

        {
            let router = provider.host_router.read().await;
            assert_eq!(router.hosts.len(), 3);
            assert_eq!(router.components.len(), 3);
            assert!(router
                .hosts
                .get("foo.com")
                .is_some_and(|target| &target.to_string() == "foo"));
            assert!(router
                .components
                .get(&(Arc::from("foo"), Arc::from("default")))
                .is_some_and(|h| &h.to_string() == "foo.com"));
            assert!(router
                .hosts
                .get("bar.com")
                .is_some_and(|target| &target.to_string() == "bar"));
            assert!(router
                .components
                .get(&(Arc::from("bar"), Arc::from("default")))
                .is_some_and(|h| &h.to_string() == "bar.com"));
            assert!(router
                .hosts
                .get("baz.com")
                .is_some_and(|target| &target.to_string() == "baz"));
            assert!(router
                .components
                .get(&(Arc::from("baz"), Arc::from("default")))
                .is_some_and(|h| &h.to_string() == "baz.com"));
        }

        // Rejecting reserved hosts / linked components
        assert!(
            provider
                .put_link(
                    "notbaz",
                    "default",
                    &HashMap::from([("host".to_string(), "baz.com".to_string())]),
                )
                .await
                .is_err(),
            "should fail to register a host that's already registered"
        );
        assert!(
            provider
                .put_link(
                    "baz",
                    "default",
                    &HashMap::from([("host".to_string(), "notbaz.com".to_string())]),
                )
                .await
                .is_err(),
            "should fail to register a host to a component that already has a host"
        );

        // Delete host registrations
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
            let router = provider.host_router.read().await;
            assert!(router.hosts.is_empty());
            assert!(router.components.is_empty());
        }

        Ok(())
    }
}
