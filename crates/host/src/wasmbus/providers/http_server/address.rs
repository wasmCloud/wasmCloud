use core::net::SocketAddr;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use http::header::HOST;
use http::uri::Scheme;
use http::Uri;
use http_body_util::BodyExt as _;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::{info_span, instrument, trace_span, Instrument as _, Span};
use wasmcloud_core::http::{load_settings, ServiceSettings};
use wasmcloud_tracing::KeyValue;
use wrpc_interface_http::ServeIncomingHandlerWasmtime as _;

use crate::bindings::exports::wrpc::extension;
use crate::bindings::wrpc::extension::types::{
    BaseConfig, BindRequest, BindResponse, HealthCheckResponse, InterfaceConfig,
};

use crate::wasmbus::{Component, InvocationContext};

use super::listen;

#[derive(Clone)]
pub(crate) struct Provider {
    /// Default address for the provider to try to listen on if no address is provided
    pub(crate) address: SocketAddr,
    /// Map of components that the provider can instantiate, keyed by component ID
    pub(crate) components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
    /// Map of links that the provider has established, component ID -> link name -> listener task
    pub(crate) links: Arc<Mutex<HashMap<Arc<str>, HashMap<Box<str>, JoinSet<()>>>>>,
    pub(crate) lattice_id: Arc<str>,
    pub(crate) host_id: Arc<str>,
    /// Broadcast sender for shutdown signaling
    pub(crate) quit_tx: broadcast::Sender<()>,
}

impl extension::manageable::Handler<Option<wasmcloud_provider_sdk::Context>> for Provider {
    async fn bind(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        Ok(Ok(BindResponse {
            identity_token: None,
            provider_pubkey: None,
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        Ok(Ok(HealthCheckResponse {
            healthy: true,
            message: Some("OK".to_string()),
        }))
    }

    /// Handle shutdown request by signaling the provider to shut down
    async fn shutdown(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
    ) -> anyhow::Result<Result<(), String>> {
        // NOTE: The result is ignored because the channel will be closed if the last
        // receiver is dropped, which is a valid way to shut down.
        let _ = self.quit_tx.send(());
        Ok(Ok(()))
    }
}

impl extension::configurable::Handler<Option<wasmcloud_provider_sdk::Context>> for Provider {
    #[instrument(level = "debug", skip_all)]
    async fn update_base_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _config: BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn update_interface_export_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _source_id: String,
        _link_name: String,
        _config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
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

        let ServiceSettings { address, .. } =
            load_settings(Some(self.address), &config_map).context("failed to load settings")?;

        let components = Arc::clone(&self.components);
        let host_id = Arc::clone(&self.host_id);
        let lattice_id = Arc::clone(&self.lattice_id);
        let target_id_arc: Arc<str> = Arc::from(target_id.as_str());
        let link_name_arc: Arc<str> = Arc::from(link_name.as_str());

        let tasks = listen(address, {
            let target_id = Arc::clone(&target_id_arc);
            let link_name = Arc::clone(&link_name_arc);
            move |req: hyper::Request<hyper::body::Incoming>| {
                let components = Arc::clone(&components);
                let host_id = Arc::clone(&host_id);
                let lattice_id = Arc::clone(&lattice_id);
                let target_id = Arc::clone(&target_id);
                let link_name = Arc::clone(&link_name);
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
                    // TODO(#3705): Propagate trace context from headers
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
                                    KeyValue::new("component.id", Arc::clone(&target_id)),
                                    KeyValue::new("link.name", Arc::clone(&link_name)),
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
        .await?;

        // Store in nested structure: component_id -> link_name -> tasks
        let mut links = self
            .links
            .lock()
            .instrument(trace_span!("insert_link"))
            .await;
        links
            .entry(target_id_arc)
            .or_insert_with(HashMap::new)
            .insert(link_name.into(), tasks);

        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_interface_import_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        target_id: String,
        link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        let mut links = self.links.lock().await;
        if let Some(component_links) = links.get_mut(target_id.as_str()) {
            if let Some(mut tasks) = component_links.remove(&Box::<str>::from(link_name.as_str())) {
                tasks.shutdown().await;
            }
            // Clean up empty component entry
            if component_links.is_empty() {
                links.remove(target_id.as_str());
            }
        }
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_interface_export_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _source_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }
}
