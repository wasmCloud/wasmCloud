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
//! - CORS support (select allowed_origins, allowed_methods,
//!   allowed_headers.) Cors has sensible defaults so it should
//!   work as-is for development purposes, and may need refinement
//!   for production if a more secure configuration is required.
//! - All settings can be specified at runtime, using per-actor link settings:
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
//! Each actor that links to this provider gets
//! its own bind address (interface ip and port) and a lightweight
//! tokio thread (lighter weight than an OS thread, more like "green threads").
//! Tokio can manage a thread pool (of OS threads) to be shared
//! by the all of the server green threads.
//!

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use flume::{bounded, Receiver, Sender};
use futures::Future;
use http::HeaderMap;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, instrument, trace, warn, Instrument};
use warp::path::FullPath;
use warp::Filter;

use wasmcloud_provider_sdk::core::{LinkDefinition, WasmCloudEntity};
use wasmcloud_provider_sdk::error::{InvocationError, ProviderInvocationError};

mod hashmap_ci;
pub(crate) use hashmap_ci::make_case_insensitive;

mod settings;
pub use settings::{load_settings, ServiceSettings, CONTENT_LEN_LIMIT, DEFAULT_MAX_CONTENT_LEN};

mod warp_util;
use warp_util::{convert_request_headers, convert_response_headers, cors_filter, opt_raw_query};

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: HttpServerProvider,
    contract: "wasmcloud:httpserver",
    replace_witified_maps: true,
    exposed_interface_deny_list: [
        "wasmcloud:bus/lattice",
        "wasmcloud:bus/guest",
    ],
    wit_bindgen_cfg: "provider-http-server"
});

/// HttpServer provider implementation.
#[derive(Clone, Default)]
pub struct HttpServerProvider {
    // map to store http server (and its link parameters) for each linked actor
    actors: Arc<dashmap::DashMap<String, HttpServerCore>>,
}

/// Your provider can handle any of these methods
/// to receive notification of new actor links, deleted links,
/// and for handling health check.
/// Default handlers are implemented in the trait ProviderHandler.
#[async_trait]
impl WasmcloudCapabilityProvider for HttpServerProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        let settings = match load_settings(&ld.values) {
            Ok(s) => s,
            Err(e) => {
                error!(%e, ?ld, "httpserver failed to load settings for actor");
                return false;
            }
        };

        // Start a server instance that calls the given actor
        let http_server = HttpServerCore::new(settings.clone(), call_actor);
        if let Err(e) = http_server.start(ld).await {
            error!(%e, ?ld, "httpserver failed to start listener for actor");
            return false;
        }

        // Save the actor and server instance locally
        self.actors.insert(ld.actor_id.to_string(), http_server);

        true
    }

    /// Handle notification that a link is dropped - stop the http listener
    async fn delete_link(&self, actor_id: &str) {
        if let Some(entry) = self.actors.remove(actor_id) {
            info!(%actor_id, "httpserver stopping listener for actor");
            entry.1.begin_shutdown();
        }
    }

    /// Handle shutdown request by shutting down all the http server threads
    async fn shutdown(&self) {
        // empty the actor link data and stop all servers
        self.actors.clear();
    }
}

////////////
// Server //
////////////

const HANDLE_REQUEST_METHOD: &str = "HttpServer.HandleRequest";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query_string: String,
    pub header: ::std::collections::HashMap<String, Vec<String>>,
    #[serde(with = "::serde_bytes")]
    pub body: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpResponse {
    pub status_code: u16,
    pub header: ::std::collections::HashMap<String, Vec<String>>,
    #[serde(with = "::serde_bytes")]
    pub body: Vec<u8>,
}

pub struct Server<'a> {
    ld: &'a LinkDefinition,
    timeout: Option<std::time::Duration>,
}

impl<'a> Server<'a> {
    pub fn new(ld: &'a LinkDefinition, timeout: Option<Duration>) -> Self {
        Self { ld, timeout }
    }

    pub async fn handle_request(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, ProviderInvocationError> {
        let connection = wasmcloud_provider_sdk::provider_main::get_connection();

        let client = connection.get_rpc_client();
        let origin = WasmCloudEntity {
            public_key: self.ld.provider_id.clone(),
            link_name: self.ld.link_name.clone(),
            contract_id: "wasmcloud:httpserver".to_string(),
        };
        let target = WasmCloudEntity {
            public_key: self.ld.actor_id.clone(),
            ..Default::default()
        };

        let data = wasmcloud_provider_sdk::serialize(&req)?;

        let response = if let Some(timeout) = self.timeout {
            client
                .send_timeout(origin, target, HANDLE_REQUEST_METHOD, data, timeout)
                .await?
        } else {
            client
                .send(origin, target, HANDLE_REQUEST_METHOD, data)
                .await?
        };

        if let Some(e) = response.error {
            return Err(ProviderInvocationError::Provider(e));
        }

        let response: HttpResponse = wasmcloud_provider_sdk::deserialize(&response.msg)?;

        Ok(response)
    }
}

/// Forward a [`Request`] to an Actor.
#[instrument(level = "debug", skip_all, fields(actor_id = %ld.actor_id))]
async fn call_actor(
    ld: Arc<LinkDefinition>,
    req: HttpRequest,
    timeout: Option<std::time::Duration>,
) -> Result<HttpResponse, ProviderInvocationError> {
    let sender = Server::new(&ld, timeout);

    let rc = sender.handle_request(req).await;
    match rc {
        Err(ProviderInvocationError::Invocation(InvocationError::Timeout)) => {
            error!("actor request timed out: returning 503",);
            Ok(HttpResponse {
                status_code: 503,
                body: Default::default(),
                header: Default::default(),
            })
        }

        Ok(resp) => {
            trace!(
                status_code = %resp.status_code,
                "http response received from actor"
            );
            Ok(resp)
        }
        Err(e) => {
            warn!(
                error = %e,
                "actor responded with error"
            );
            Err(e)
        }
    }
}

//////////
// Util //
//////////

/// Errors generated by this HTTP server
#[derive(Debug, thiserror::Error)]
pub enum HttpServerError {
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("problem reading settings: {0}")]
    Settings(String),

    #[error("provider startup: {0}")]
    Init(String),

    #[error("warp error: {0}")]
    Warp(warp::Error),

    #[error("deserializing settings: {0}")]
    SettingsToml(toml::de::Error),
}

/// Alias for functions that trigger an actor
pub type AsyncCallActorFn = Box<
    dyn Fn(
            Arc<LinkDefinition>,
            HttpRequest,
            Option<Duration>,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<HttpResponse, ProviderInvocationError>> + Send + 'static,
            >,
        > + Send
        + Sync,
>;

/// Wrapper for functions that trigger action calls for trait implementation
struct CallActorFn(AsyncCallActorFn);

impl CallActorFn {
    fn call(
        &self,
        ld: Arc<LinkDefinition>,
        req: HttpRequest,
        timeout: Option<Duration>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, ProviderInvocationError>> + Send + 'static>>
    {
        Box::pin((self.0.as_ref())(ld, req, timeout))
    }
}

/// Inner configuration holder for [`HttpServerCore`]
pub struct Inner {
    settings: ServiceSettings,
    shutdown_tx: Sender<bool>,
    shutdown_rx: Receiver<bool>,
    call_actor: CallActorFn,
}

/// An asynchronous HttpServer with support for CORS and TLS
///
/// ```no_test
///   use wasmcloud_provider_httpserver::{HttpServer, load_settings};
///   let settings = load_settings(ld.values)?;
///   let server = HttpServer::new(settings);
///   let task = server.serve()?;
///   tokio::task::spawn(task);
/// ```
#[derive(Clone)]
pub struct HttpServerCore {
    inner: Arc<Inner>,
}

impl std::ops::Deref for HttpServerCore {
    type Target = Inner;
    fn deref(&self) -> &Self::Target {
        self.inner.as_ref()
    }
}

impl HttpServerCore {
    /// Initializes server with settings
    pub fn new<F, Fut>(settings: ServiceSettings, call_actor_fn: F) -> Self
    where
        F: Fn(Arc<LinkDefinition>, HttpRequest, Option<Duration>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<HttpResponse, ProviderInvocationError>> + 'static + Send,
    {
        let (shutdown_tx, shutdown_rx) = bounded(1);
        let call_actor_fn = Arc::new(call_actor_fn);
        Self {
            inner: Arc::new(Inner {
                settings,
                shutdown_tx,
                shutdown_rx,
                call_actor: CallActorFn(Box::new(
                    move |ld: Arc<LinkDefinition>, req: HttpRequest, timeout: Option<Duration>| {
                        let call_actor_fn = call_actor_fn.clone();
                        Box::pin(call_actor_fn(ld, req, timeout))
                    },
                )),
            }),
        }
    }

    /// Initiate server shutdown. This can be called from any thread and is non-blocking.
    pub fn begin_shutdown(&self) {
        let _ = self.shutdown_tx.try_send(true);
    }

    /// Start the server in a new thread
    /// ```no_test
    ///    use wasmcloud_provider_httpserver::{HttpServer, load_settings};
    ///    let settings = load_settings(&ld.values)?;
    ///    let server = HttpServer::new(settings);
    ///    let _ = server.start().await?;
    /// ```
    pub async fn start(&self, ld: &LinkDefinition) -> Result<JoinHandle<()>, HttpServerError> {
        let timeout = self
            .inner
            .settings
            .timeout_ms
            .map(std::time::Duration::from_millis);

        let ld = Arc::new(ld.clone());
        let linkdefs = ld.clone();
        let trace_ld = ld.clone();
        let arc_inner = self.inner.clone();
        let route = warp::any()
            .and(warp::header::headers_cloned())
            .and(warp::method())
            .and(warp::body::bytes())
            .and(warp::path::full())
            .and(opt_raw_query())
            .and_then(
                move |
                      headers: HeaderMap,
                      method: http::method::Method,
                      body: Bytes,
                      path: FullPath,
                      query: String| {
                    let span = tracing::debug_span!("http request", %method, path = %path.as_str(), %query);
                    let ld = linkdefs.clone();
                    let arc_inner = arc_inner.clone();
                    async move{
                        if let Some(readonly_mode) = arc_inner.settings.readonly_mode{
                            if readonly_mode && method!= http::method::Method::GET && method!= http::method::Method::HEAD {
                                debug!("Cannot use other methods in Read Only Mode");
                                // If this fails it is developer error, so unwrap is okay
                                let resp = http::Response::builder().status(http::StatusCode::METHOD_NOT_ALLOWED).body(Vec::with_capacity(0)).unwrap();
                                return Ok::<_, warp::Rejection>(resp)
                            }
                        }
                        let hmap = convert_request_headers(&headers);
                        let req = HttpRequest {
                            body: Vec::from(body),
                            header: hmap,
                            method: method.as_str().to_ascii_uppercase(),
                            path: path.as_str().to_string(),
                            query_string: query,
                        };
                        trace!(
                            ?req,
                            "httpserver calling actor"
                        );
                        let response = match arc_inner.call_actor.call(ld.clone(), req, timeout).in_current_span().await {
                            Ok(resp) => resp,
                            Err(e) => {
                                error!(
                                    error = %e,
                                    "Error sending Request to actor"
                                );
                                HttpResponse {
                                    status_code: http::StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                                    body: Default::default(),
                                    header: Default::default(),
                                }
                            }
                        };
                        let status = match http::StatusCode::from_u16(response.status_code) {
                            Ok(status_code) => status_code,
                            Err(e) => {
                                error!(
                                    status_code = %response.status_code,
                                    error = %e,
                                    "invalid response status code, changing to 500"
                                );
                                http::StatusCode::INTERNAL_SERVER_ERROR
                            }
                        };
                        let http_builder = http::Response::builder()
                        .status(status);
                        let http_builder = if let Some(cache_control_header) = arc_inner.settings.cache_control.as_ref(){
                            let mut builder = http_builder;
                            builder = builder.header("Cache-Control",cache_control_header);
                            builder
                        }else{
                            http_builder
                        };
                        // Unwrapping here because validation takes place for the linkdef
                        let mut http_response = http_builder.body(response.body).unwrap();
                        convert_response_headers(response.header, http_response.headers_mut());
                        Ok::<_, warp::Rejection>(http_response)
                    }.instrument(span)
                },
            ).with(warp::trace(move |req_info| {
                let actor_id = &trace_ld.actor_id;
                let span = tracing::debug_span!("request", method = %req_info.method(), path = %req_info.path(), query = tracing::field::Empty, %actor_id);
                if let Some(remote_addr) = req_info.remote_addr() {
                    span.record("remote_addr", &tracing::field::display(remote_addr));
                }

                span
            }));

        let addr = self.settings.address.unwrap();
        info!(
            %addr,
            actor_id = %ld.actor_id,
            "httpserver starting listener for actor",
        );

        // add Cors configuration, if enabled, and spawn either TlsServer or Server
        let cors = cors_filter(&self.settings)?;
        let server = warp::serve(route.with(cors));
        let handle = tokio::runtime::Handle::current();
        let shutdown_rx = self.shutdown_rx.clone();
        let join = if self.settings.tls.is_set() {
            let (_, fut) = server
                .tls()
                // unwrap ok here because tls.is_set confirmed both fields are some()
                .key_path(self.settings.tls.priv_key_file.as_ref().unwrap())
                .cert_path(self.settings.tls.cert_file.as_ref().unwrap())
                // we'd prefer to use try_bind_with_graceful_shutdown but it's not supported
                // for tls server yet. Waiting on https://github.com/seanmonstar/warp/pull/717
                // attempt to bind to the address
                .bind_with_graceful_shutdown(addr, async move {
                    if let Err(err) = shutdown_rx.recv_async().await {
                        error!(%err, "shutting down httpserver listener");
                    }
                });
            handle.spawn(fut)
        } else {
            let (_, fut) = server
                .try_bind_with_graceful_shutdown(addr, async move {
                    if let Err(err) = shutdown_rx.recv_async().await {
                        error!(%err, "shutting down httpserver listener");
                    }
                })
                .map_err(|e| {
                    HttpServerError::Settings(format!(
                        "failed binding to address '{}' reason: {}",
                        &addr.to_string(),
                        e
                    ))
                })?;
            handle.spawn(fut)
        };

        Ok(join)
    }
}

impl Drop for HttpServerCore {
    /// Drop the client connection. Does not block or fail if the client has already been closed.
    fn drop(&mut self) {
        let _ = self.shutdown_tx.try_send(true);
    }
}
