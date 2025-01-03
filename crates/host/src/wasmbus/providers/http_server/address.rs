use core::net::SocketAddr;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use http::header::HOST;
use http::uri::Scheme;
use http::Uri;
use http_body_util::BodyExt as _;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{info_span, instrument, trace_span, Instrument as _, Span};
use wasmcloud_provider_http_server::{load_settings, ServiceSettings};
use wasmcloud_provider_sdk::{LinkConfig, LinkDeleteInfo};
use wasmcloud_tracing::KeyValue;
use wrpc_interface_http::ServeIncomingHandlerWasmtime as _;

use crate::wasmbus::{Component, InvocationContext};

use super::listen;

pub(crate) struct Provider {
    pub(crate) address: SocketAddr,
    pub(crate) components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
    pub(crate) links: Mutex<HashMap<Arc<str>, HashMap<Box<str>, JoinHandle<()>>>>,
    pub(crate) lattice_id: Arc<str>,
    pub(crate) host_id: Arc<str>,
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

        let lattice_id = Arc::clone(&self.lattice_id);
        let host_id = Arc::clone(&self.host_id);
        let components = Arc::clone(&self.components);
        let target_id: Arc<str> = Arc::from(target_id);
        let target_id_fn = Arc::clone(&target_id);
        let task = listen(
            address,
            move |req: hyper::Request<hyper::body::Incoming>| {
                let target_id = Arc::clone(&target_id_fn);
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
        .await?;

        self.links
            .lock()
            .instrument(trace_span!("insert_link"))
            .await
            .entry(target_id)
            .or_default()
            .insert(link_name.into(), task);
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
