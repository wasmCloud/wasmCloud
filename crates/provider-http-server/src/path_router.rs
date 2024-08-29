use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use axum::extract::{self};
use axum::handler::Handler;
use axum_server::tls_rustls::RustlsConfig;
use axum_server::Handle;
use futures::Stream;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::{spawn, time};
use tracing::{debug, error, info, instrument, trace};
use wasmcloud_provider_sdk::{get_connection, LinkConfig, LinkDeleteInfo, Provider};
use wrpc_interface_http::InvokeIncomingHandler as _;

use crate::{get_cors_layer, get_tcp_listener, ResponseBody, ServiceSettings};

/// `wrpc:http/incoming-handler` provider implementation.
#[derive(Clone)]
pub struct HttpServerProvider {
    /// Map from a path to the component ID that is handling that path
    router: Arc<RwLock<HashMap<String, String>>>,
    /// Reverse lookup to find the path for a (component,link_name) pair
    paths: Arc<RwLock<HashMap<(String, String), String>>>,
    /// Handle to the server task
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
    pub(crate) async fn new(settings: Arc<ServiceSettings>) -> anyhow::Result<Self> {
        let router = Arc::new(RwLock::new(HashMap::new()));
        let paths = Arc::new(RwLock::new(HashMap::new()));

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
        let task_router = router.clone();
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
                                timeout_ms: settings.timeout_ms,
                                readonly_mode: settings.readonly_mode,
                                cache_control: settings.cache_control.clone(),
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
                                timeout_ms: settings.timeout_ms,
                                readonly_mode: settings.readonly_mode,
                                cache_control: settings.cache_control.clone(),
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
            router,
            paths,
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
        // TODO: error log
        let path = link_config.config.get("path").context("Missing path")?;

        // NOTE(brooksmtownsend): Using blocks here to ensure we are never holding both the
        // router and paths write locks at the same time

        // TODO: reevaluate with entry API
        // Insert the path into the router
        {
            let mut router = self.router.write().await;
            if router.contains_key(path.as_str()) {
                // When we can return errors from links, tell the host this was invalid
                bail!("Path {path} already in use by a different componnet");
            }
            router.insert(path.to_string(), link_config.target_id.to_string());
        }
        // Insert the path into the paths map for future lookups
        {
            let mut paths = self.paths.write().await;
            if paths.contains_key(&(
                link_config.target_id.to_string(),
                link_config.link_name.to_string(),
            )) {
                // When we can return errors from links, tell the host this was invalid
                bail!("Path {path} already in use by a different componnet/link_name pair");
            }
            paths.insert(
                (
                    link_config.target_id.to_string(),
                    link_config.link_name.to_string(),
                ),
                path.to_string(),
            );
        }

        Ok(())
    }

    /// Remove the path for a particular component/link_name pair
    #[instrument(level = "info", skip_all, fields(target_id = info.get_target_id()))]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_target_id();
        let link_name = info.get_link_name();

        // Using blocks here to ensure we never hold both write locks at the same time
        let path = {
            let mut paths = self.paths.write().await;
            paths.remove(&(component_id.to_string(), link_name.to_string()))
        };
        if let Some(path) = path {
            self.router.write().await.remove(&path);
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
    router: Arc<RwLock<HashMap<String, String>>>,
    scheme: http::uri::Scheme,
    timeout_ms: Option<u64>,
    readonly_mode: Option<bool>,
    cache_control: Option<String>,
}

// TODO: Reduce code duplication
#[instrument(level = "debug", skip(router))]
async fn handle_request(
    extract::State(RequestContext {
        router,
        scheme,
        timeout_ms,
        readonly_mode,
        cache_control,
    }): extract::State<RequestContext>,
    extract::Host(authority): extract::Host,
    request: extract::Request,
) -> impl axum::response::IntoResponse {
    let timeout = timeout_ms.map(Duration::from_millis);
    let method = request.method();
    if let Some(readonly_mode) = readonly_mode {
        if readonly_mode
            && method != http::method::Method::GET
            && method != http::method::Method::HEAD
        {
            debug!("only GET and HEAD allowed in read-only mode");
            Err((
                http::StatusCode::METHOD_NOT_ALLOWED,
                "only GET and HEAD allowed in read-only mode",
            ))?;
        }
    }
    let (
        http::request::Parts {
            method,
            uri,
            headers,
            ..
        },
        body,
    ) = request.into_parts();
    let http::uri::Parts { path_and_query, .. } = uri.into_parts();

    let Some(path) = path_and_query.as_ref().map(|p_and_q| p_and_q.path()) else {
        Err((http::StatusCode::BAD_REQUEST, "missing path"))?
    };

    let Some(target_component) = router.read().await.get(path).cloned() else {
        Err((http::StatusCode::NOT_FOUND, "path not found"))?
    };

    let mut uri = http::Uri::builder().scheme(scheme);
    if !authority.is_empty() {
        uri = uri.authority(authority);
    }
    if let Some(path_and_query) = path_and_query {
        uri = uri.path_and_query(path_and_query);
    }
    let uri = uri
        .build()
        .map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let mut req = http::Request::builder();
    *req.headers_mut().ok_or((
        http::StatusCode::INTERNAL_SERVER_ERROR,
        "invalid request generated",
    ))? = headers;
    let req = req
        .uri(uri)
        .method(method)
        .body(body)
        .map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    // Create a new wRPC client with all headers from the current span injected
    let mut cx = async_nats::HeaderMap::new();
    for (k, v) in
        wasmcloud_provider_sdk::wasmcloud_tracing::context::TraceContextInjector::default_with_span(
        )
        .iter()
    {
        cx.insert(k.as_str(), v.as_str())
    }

    let wrpc = get_connection().get_wrpc_client_custom(&target_component, None);
    trace!(?req, "httpserver calling component");
    let fut = wrpc.invoke_handle_http(Some(cx), req);
    let res = if let Some(timeout) = timeout {
        let Ok(res) = time::timeout(timeout, fut).await else {
            Err(http::StatusCode::REQUEST_TIMEOUT)?
        };
        res
    } else {
        fut.await
    };
    let (res, errors, io) =
        res.map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}")))?;
    let io = io.map(spawn);
    let errors: Box<dyn Stream<Item = _> + Send + Unpin> = Box::new(errors);
    // TODO: Convert this to http status code
    let mut res =
        res.map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, format!("{err:?}")))?;
    if let Some(cache_control) = cache_control.as_ref() {
        let cache_control = http::HeaderValue::from_str(cache_control)
            .map_err(|err| (http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
        res.headers_mut().append("Cache-Control", cache_control);
    };
    axum::response::Result::<_, axum::response::ErrorResponse>::Ok(res.map(|body| ResponseBody {
        body,
        errors,
        io,
    }))
}
