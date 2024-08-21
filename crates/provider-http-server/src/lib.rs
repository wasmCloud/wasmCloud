//! The httpserver capability provider allows wasmcloud actors to receive
//! and process http(s) messages from web browsers, command-line tools
//! such as curl, and other http clients. The server is fully asynchronous,
//! and built on Rust's high-performance warp engine, which is in turn based
//! on hyper, and can process a large number of simultaneous connections.
//!
//! ## Features:
//!
//! - HTTP/1 and HTTP/2
//! - TLS
//! - CORS support (select `allowed_origins`, `allowed_methods`,
//!   `allowed_headers`.) Cors has sensible defaults so it should
//!   work as-is for development purposes, and may need refinement
//!   for production if a more secure configuration is required.
//! - All settings can be specified at runtime, using per-component link settings:
//!   - bind interface/port
//!   - logging level
//!   - TLS
//!   - Cors
//! - Flexible confiuration loading: from host, or from local toml or json file.
//! - Fully asynchronous, using tokio lightweight "green" threads
//! - Thread pool (for managing a pool of OS threads). The default
//!   thread pool has one thread per cpu core.
//! - Packaged as a rust library crate for implementation flexibility
//!
//! ## More tech info:
//!
//! Each component that links to this provider gets
//! its own bind address (interface ip and port) and a lightweight
//! tokio thread (lighter weight than an OS thread, more like "green threads").
//! Tokio can manage a thread pool (of OS threads) to be shared
//! by the all of the server green threads.
//!

use core::future::Future;
use core::pin::Pin;
use core::str::FromStr as _;
use core::task::{ready, Context, Poll};
use core::time::Duration;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{anyhow, Context as _};
use axum::extract;
use axum::handler::Handler as _;
use axum_server::tls_rustls::RustlsConfig;
use bytes::Bytes;
use futures::Stream;
use pin_project_lite::pin_project;
use tokio::task::JoinHandle;
use tokio::{spawn, time};
use tower_http::cors::{self, CorsLayer};
use tracing::{debug, error, info, instrument, trace};
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, run_provider, LinkConfig, LinkDeleteInfo, Provider,
};
use wrpc_interface_http::InvokeIncomingHandler as _;

mod hashmap_ci;
pub(crate) use hashmap_ci::make_case_insensitive;

mod settings;
pub use settings::{load_settings, ServiceSettings, CONTENT_LEN_LIMIT, DEFAULT_MAX_CONTENT_LEN};

use crate::settings::Tls;

/// `wrpc:http/incoming-handler` provider implementation.
#[derive(Clone, Default)]
pub struct HttpServerProvider {
    // map to store http server (and its link parameters) for each linked component
    actors: Arc<dashmap::DashMap<String, HttpServerCore>>,
}

pub async fn run() -> anyhow::Result<()> {
    initialize_observability!(
        "http-server-provider",
        std::env::var_os("PROVIDER_HTTP_SERVER_FLAMEGRAPH_PATH")
    );
    let shutdown = run_provider(HttpServerProvider::default(), "http-server-provider")
        .await
        .context("failed to run provider")?;
    shutdown.await;
    Ok(())
}

impl Provider for HttpServerProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-component resources, and checking authorization.
    async fn receive_link_config_as_source(
        &self,
        link_config: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let settings = match load_settings(link_config.config)
            .context("httpserver failed to load settings for component")
        {
            Ok(settings) => settings,
            Err(e) => {
                error!(
                    config = ?link_config.config,
                    "httpserver failed to load settings for component: {}", e.to_string()
                );
                return Err(e);
            }
        };

        // Start a server instance that calls the given component
        let http_server = HttpServerCore::new(Arc::new(settings), link_config.target_id)
            .await
            .context("httpserver failed to start listener for component")?;

        // Save the component and server instance locally
        self.actors
            .insert(link_config.target_id.to_string(), http_server);

        Ok(())
    }

    /// Handle notification that a link is dropped - stop the http listener
    #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id()))]
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_source_id();
        if let Some((_, server)) = self.actors.remove(component_id) {
            info!(component_id, "httpserver stopping listener for component");
            server.handle.shutdown();
        }
        Ok(())
    }

    /// Handle shutdown request by shutting down all the http server threads
    async fn shutdown(&self) -> anyhow::Result<()> {
        // empty the component link data and stop all servers
        self.actors.clear();
        Ok(())
    }
}

/// Errors generated by this HTTP server
#[derive(Debug, thiserror::Error)]
pub enum HttpServerError {
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("problem reading settings: {0}")]
    Settings(String),

    #[error("provider startup: {0}")]
    Init(String),

    #[error("axum error: {0}")]
    Axum(axum::Error),

    #[error("deserializing settings: {0}")]
    SettingsToml(toml::de::Error),
}

/// An asynchronous `wrpc:http/incoming-handler` with support for CORS and TLS
///
/// ```no_test
///   use wasmcloud_provider_httpserver::{HttpServer, load_settings};
///   let settings = load_settings(ld.values)?;
///   let server = HttpServer::new(settings);
///   let task = server.serve()?;
///   tokio::task::spawn(task);
/// ```
pub struct HttpServerCore {
    /// The handle to the server handling incoming requests
    handle: axum_server::Handle,
    /// The asynchronous task running the server
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone, Debug)]
struct RequestContext {
    target: String,
    settings: Arc<ServiceSettings>,
    scheme: http::uri::Scheme,
}

pin_project! {
    struct ResponseBody {
        #[pin]
        body: wrpc_interface_http::HttpBody,
        #[pin]
        errors: Box<dyn Stream<Item = wrpc_interface_http::HttpBodyError<axum::Error>> + Send + Unpin>,
        #[pin]
        io: Option<JoinHandle<anyhow::Result<()>>>,
    }
}

impl http_body::Body for ResponseBody {
    type Data = Bytes;
    type Error = anyhow::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let mut this = self.as_mut().project();
        if let Some(io) = this.io.as_mut().as_pin_mut() {
            match io.poll(cx) {
                Poll::Ready(Ok(Ok(()))) => {
                    this.io.take();
                }
                Poll::Ready(Ok(Err(err))) => {
                    return Poll::Ready(Some(Err(
                        anyhow!(err).context("failed to complete async I/O")
                    )))
                }
                Poll::Ready(Err(err)) => {
                    return Poll::Ready(Some(Err(anyhow!(err).context("I/O task failed"))))
                }
                Poll::Pending => {}
            }
        }
        match this.errors.poll_next(cx) {
            Poll::Ready(Some(err)) => {
                if let Some(io) = this.io.as_pin_mut() {
                    io.abort()
                }
                return Poll::Ready(Some(Err(anyhow!(err).context("failed to process body"))));
            }
            Poll::Ready(None) | Poll::Pending => {}
        }
        match ready!(this.body.poll_frame(cx)) {
            Some(Ok(frame)) => Poll::Ready(Some(Ok(frame))),
            Some(Err(err)) => {
                if let Some(io) = this.io.as_pin_mut() {
                    io.abort()
                }
                Poll::Ready(Some(Err(err)))
            }
            None => {
                if let Some(io) = this.io.as_pin_mut() {
                    io.abort()
                }
                Poll::Ready(None)
            }
        }
    }
}

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
    let method = request.method();
    if let Some(readonly_mode) = settings.readonly_mode {
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

    let wrpc = get_connection().get_wrpc_client_custom(target.as_str(), None);
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
    if let Some(cache_control) = settings.cache_control.as_ref() {
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

impl HttpServerCore {
    #[instrument]
    pub async fn new(settings: Arc<ServiceSettings>, target: &str) -> anyhow::Result<Self> {
        let addr = settings
            .address
            .unwrap_or_else(|| (Ipv4Addr::UNSPECIFIED, 8000).into());
        info!(
            %addr,
            %target,
            "httpserver starting listener for target",
        );

        let allow_origin = settings.cors.allowed_origins.as_ref();
        let allow_origin: Vec<_> = allow_origin
            .map(|origins| {
                origins
                    .iter()
                    .map(AsRef::as_ref)
                    .map(http::HeaderValue::from_str)
                    .collect::<Result<_, _>>()
                    .context("failed to parse allowed origins")
            })
            .transpose()?
            .unwrap_or_default();
        let allow_origin = if allow_origin.is_empty() {
            cors::AllowOrigin::any()
        } else {
            cors::AllowOrigin::list(allow_origin)
        };
        let allow_headers = settings.cors.allowed_headers.as_ref();
        let allow_headers: Vec<_> = allow_headers
            .map(|headers| {
                headers
                    .iter()
                    .map(AsRef::as_ref)
                    .map(http::HeaderName::from_str)
                    .collect::<Result<_, _>>()
                    .context("failed to parse allowed header names")
            })
            .transpose()?
            .unwrap_or_default();
        let allow_headers = if allow_headers.is_empty() {
            cors::AllowHeaders::any()
        } else {
            cors::AllowHeaders::list(allow_headers)
        };
        let allow_methods = settings.cors.allowed_methods.as_ref();
        let allow_methods: Vec<_> = allow_methods
            .map(|methods| {
                methods
                    .iter()
                    .map(AsRef::as_ref)
                    .map(http::Method::from_str)
                    .collect::<Result<_, _>>()
                    .context("failed to parse allowed methods")
            })
            .transpose()?
            .unwrap_or_default();
        let allow_methods = if allow_methods.is_empty() {
            cors::AllowMethods::any()
        } else {
            cors::AllowMethods::list(allow_methods)
        };
        let expose_headers = settings.cors.exposed_headers.as_ref();
        let expose_headers: Vec<_> = expose_headers
            .map(|headers| {
                headers
                    .iter()
                    .map(AsRef::as_ref)
                    .map(http::HeaderName::from_str)
                    .collect::<Result<_, _>>()
                    .context("failed to parse exposeed header names")
            })
            .transpose()?
            .unwrap_or_default();
        let expose_headers = if expose_headers.is_empty() {
            cors::ExposeHeaders::any()
        } else {
            cors::ExposeHeaders::list(expose_headers)
        };
        let mut cors = CorsLayer::new()
            .allow_origin(allow_origin)
            .allow_headers(allow_headers)
            .allow_methods(allow_methods)
            .expose_headers(expose_headers);
        if let Some(max_age) = settings.cors.max_age_secs {
            cors = cors.max_age(Duration::from_secs(max_age));
        }
        let service = handle_request.layer(cors);

        let settings = Arc::clone(&settings);
        let handle = axum_server::Handle::new();

        let task_handle = handle.clone();
        let target = target.to_owned();
        let socket = match &addr {
            SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4(),
            SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6(),
        }
        .context("Unable to open socket")?;
        // Copied this option from
        // https://github.com/bytecodealliance/wasmtime/blob/05095c18680927ce0cf6c7b468f9569ec4d11bd7/src/commands/serve.rs#L319.
        // This does increase throughput by 10-15% which is why we're creating the socket. We're
        // using the tokio one because it exposes the `reuseaddr` option.
        socket
            .set_reuseaddr(!cfg!(windows))
            .context("Error when setting socket to reuseaddr")?;
        socket.bind(addr).context("Unable to bind to address")?;
        let listener = socket.listen(1024).context("unable to listen on socket")?;
        let listener = listener.into_std().context("Unable to get listener")?;

        let task = if let Tls {
            cert_file: Some(crt),
            priv_key_file: Some(key),
        } = &settings.tls
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
