//! HTTP server plugin for handling incoming HTTP requests.
//!
//! This plugin implements the `wasi:http/incoming-handler` interface, allowing
//! WebAssembly components to handle HTTP requests. It provides a complete HTTP
//! server implementation with support for:
//!
//! - Virtual hosting based on Host headers
//! - TLS/HTTPS connections
//! - Component isolation per request
//! - Graceful shutdown capabilities
//!
//! # Architecture
//!
//! The HTTP server plugin works by:
//! 1. Binding to a TCP socket and listening for connections
//! 2. Routing requests to components based on the Host header
//! 3. Creating isolated component instances for each request
//! 4. Managing the request/response lifecycle through WASI-HTTP
//! ```

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::Duration,
};

use crate::engine::ctx::SharedCtx;
use crate::engine::workload::ResolvedWorkload;
use crate::wit::WitInterface;
use anyhow::{Context, ensure};
use http_body_util::BodyExt;
use hyper::client::conn::http2;
use hyper_util::{rt::TokioExecutor, server::conn::auto};
use opentelemetry::context::FutureExt;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, error, info, instrument, warn};
use wasmtime::Store;
use wasmtime::component::InstancePre;
use wasmtime_wasi_http::{
    WasiHttpView,
    bindings::{ProxyPre, http::types::Scheme},
    body::HyperOutgoingBody,
    hyper_request_error,
    io::TokioIo,
    types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
};

use rustls::{ServerConfig, pki_types::CertificateDer};
use rustls_pemfile::{certs, private_key};
use tokio::sync::{RwLock, mpsc};
use tokio_rustls::TlsAcceptor;

/// Trait defining the routing behavior for HTTP requests
/// Allows for custom routing logic based on workload IDs and requests
/// Use this trait to implement custom routing strategies with the default HTTP Extension
#[async_trait::async_trait]
pub trait Router: Send + Sync + 'static {
    /// Register a workload that has been resolved
    /// and is guaranteed to be available for handling requests
    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()>;

    /// Unregister a workload that is being stopped
    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()>;

    /// Determine if the outgoing request is allowed
    fn allow_outgoing_request(
        &self,
        workload_id: &str,
        request: &hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: &wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> anyhow::Result<()>;

    /// Pick a workload ID based on the incoming request
    fn route_incoming_request(
        &self,
        req: &hyper::Request<hyper::body::Incoming>,
    ) -> anyhow::Result<String>;
}

/// Router that routes requests by 'Host' header, configured via WitInterface config
#[derive(Default)]
pub struct DynamicRouter {
    host_to_workload: tokio::sync::RwLock<HashMap<String, HashSet<String>>>,
    workload_to_host: tokio::sync::RwLock<HashMap<String, String>>,
}

/// Implementation of Router that maps Host headers to workload IDs
/// based on the 'host' config in the wasi:http/incoming-handler interface
#[async_trait::async_trait]
impl Router for DynamicRouter {
    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        _component_id: &str,
    ) -> anyhow::Result<()> {
        let incoming_handler_interface = WitInterface::from("wasi:http/incoming-handler");
        let Some(http_iface) = resolved_handle
            .host_interfaces()
            .iter()
            .find(|iface| iface.contains(&incoming_handler_interface))
        else {
            anyhow::bail!("workload did not request wasi:http/incoming-handler interface");
        };

        let host_header = http_iface
            .config
            .get("host")
            .cloned()
            .context("No host header found")?;

        {
            let mut lock = self.workload_to_host.write().await;
            lock.insert(resolved_handle.id().to_string(), host_header.clone());
        }

        {
            let mut lock = self.host_to_workload.write().await;
            let entry = lock.entry(host_header.clone()).or_insert_with(HashSet::new);
            entry.insert(resolved_handle.id().to_string());
        }

        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        let mut lock = self.workload_to_host.write().await;
        if let Some(host_header) = lock.remove(workload_id) {
            let mut host_lock = self.host_to_workload.write().await;
            if let Some(workload_set) = host_lock.get_mut(&host_header) {
                workload_set.remove(workload_id);
                if workload_set.is_empty() {
                    host_lock.remove(&host_header);
                }
            }
        }
        Ok(())
    }

    fn allow_outgoing_request(
        &self,
        _workload_id: &str,
        _request: &hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        _config: &wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Pick a workload ID based on the incoming request
    fn route_incoming_request(
        &self,
        req: &hyper::Request<hyper::body::Incoming>,
    ) -> anyhow::Result<String> {
        tokio::task::block_in_place(move || {
            let lock = self.host_to_workload.try_read()?;
            let workload_host = req
                .headers()
                .get(hyper::header::HOST)
                .and_then(|h| h.to_str().ok())
                .or_else(|| req.uri().authority().map(|a| a.as_str()))
                .context("no Host header or :authority in request")?;
            let Some(workload_set) = lock.get(workload_host) else {
                anyhow::bail!("no workload bound to host header: {}", workload_host);
            };

            let workload_id = workload_set
                .iter()
                .next()
                .context("no workload IDs found for host header")?;

            Ok(workload_id.clone())
        })
    }
}

/// Development router that routes all requests to the last resolved workload
#[derive(Default)]
pub struct DevRouter {
    last_workload_id: tokio::sync::Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl Router for DevRouter {
    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        _component_id: &str,
    ) -> anyhow::Result<()> {
        let mut lock = self.last_workload_id.lock().await;
        lock.replace(resolved_handle.id().to_string());
        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        let mut lock = self.last_workload_id.lock().await;
        if let Some(current_id) = &*lock
            && current_id == workload_id
        {
            let _ = lock.take();
        }
        Ok(())
    }

    fn allow_outgoing_request(
        &self,
        _workload_id: &str,
        _request: &hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        _config: &wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Pick a workload ID based on the incoming request
    fn route_incoming_request(
        &self,
        _req: &hyper::Request<hyper::body::Incoming>,
    ) -> anyhow::Result<String> {
        let lock = self.last_workload_id.try_lock()?;
        match &*lock {
            Some(id) => Ok(id.clone()),
            None => anyhow::bail!("no workload available to route request"),
        }
    }
}

/// Trait defining the behavior of a Host HTTP Extension
/// Allows for custom handling of incoming and outgoing HTTP requests
/// Use this trait to implement custom HTTP server transport
#[async_trait::async_trait]
pub trait HostHandler: Send + Sync + 'static {
    /// Start the HTTP server
    async fn start(&self) -> anyhow::Result<()>;
    /// Stop the HTTP server
    async fn stop(&self) -> anyhow::Result<()>;
    /// Get the port on which the HTTP server is listening
    fn port(&self) -> u16;

    /// Register a workload
    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()>;
    /// Unregister a workload
    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()>;

    /// Handle an outgoing HTTP request from a workload
    fn outgoing_request(
        &self,
        workload_id: &str,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse>;
}

impl std::fmt::Debug for dyn HostHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostHandler").finish()
    }
}

#[derive(Default)]
pub struct NullServer {}

#[async_trait::async_trait]
impl HostHandler for NullServer {
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn port(&self) -> u16 {
        0
    }

    async fn on_workload_resolved(
        &self,
        _resolved_handle: &ResolvedWorkload,
        _component_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn on_workload_unbind(&self, _workload_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn outgoing_request(
        &self,
        _workload_id: &str,
        _request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        _config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
        Err(wasmtime_wasi_http::HttpError::trap(anyhow::anyhow!(
            "http client not available"
        )))
    }
}

/// A map from host header to resolved workload handles and their associated component id
pub type WorkloadHandles =
    Arc<RwLock<HashMap<String, (ResolvedWorkload, InstancePre<SharedCtx>, String)>>>;

/// HTTP server plugin that handles incoming HTTP requests for WebAssembly components.
///
/// This plugin implements the `wasi:http/incoming-handler` interface and routes
/// HTTP requests to appropriate WebAssembly components based on virtual hosting.
/// It supports both HTTP and HTTPS connections with optional mutual TLS.
pub struct HttpServer<T: Router> {
    router: Arc<T>,
    addr: SocketAddr,
    workload_handles: WorkloadHandles,
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    tls_acceptor: Option<TlsAcceptor>,
    listener: Arc<tokio::sync::Mutex<Option<TcpListener>>>,
}

impl<T: Router> std::fmt::Debug for HttpServer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServer")
            .field("addr", &self.addr)
            .finish()
    }
}

impl<T: Router> HttpServer<T> {
    /// Creates a new HTTP server that eagerly binds to the specified address.
    ///
    /// The socket is bound immediately so the port is reserved. Use port `0`
    /// to let the OS pick a free port, then call [`addr()`](Self::addr) to
    /// discover the actual address.
    ///
    /// # Arguments
    /// * `router` - The router implementation for handling requests
    /// * `addr` - The socket address to bind to
    pub async fn new(router: T, addr: SocketAddr) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        Ok(Self {
            router: Arc::new(router),
            addr,
            workload_handles: Arc::default(),
            shutdown_tx: Arc::new(RwLock::new(None)),
            tls_acceptor: None,
            listener: Arc::new(tokio::sync::Mutex::new(Some(listener))),
        })
    }

    /// Returns the actual bound address (useful when binding to port 0).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Creates a new HTTPS server with TLS support.
    ///
    /// # Arguments
    /// * `router` - The router implementation for handling requests
    /// * `addr` - The socket address to bind to
    /// * `cert_path` - Path to the TLS certificate file
    /// * `key_path` - Path to the private key file
    /// * `ca_path` - Optional path to CA certificate for mutual TLS
    ///
    /// # Returns
    /// A new `HttpServer` instance configured for HTTPS connections.
    ///
    /// # Errors
    /// Returns an error if the TLS configuration cannot be loaded.
    pub async fn new_with_tls(
        router: T,
        addr: SocketAddr,
        cert_path: &Path,
        key_path: &Path,
        ca_path: Option<&Path>,
    ) -> anyhow::Result<Self> {
        let tls_config = load_tls_config(cert_path, key_path, ca_path).await?;
        let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        Ok(Self {
            router: Arc::new(router),
            addr,
            workload_handles: Arc::default(),
            shutdown_tx: Arc::new(RwLock::new(None)),
            tls_acceptor: Some(tls_acceptor),
            listener: Arc::new(tokio::sync::Mutex::new(Some(listener))),
        })
    }
}

#[async_trait::async_trait]
impl<T: Router> HostHandler for HttpServer<T> {
    async fn start(&self) -> anyhow::Result<()> {
        let addr = self.addr;
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let shutdown_tx_clone = self.shutdown_tx.clone();
        let workload_handles = self.workload_handles.clone();
        let tls_acceptor = self.tls_acceptor.clone();

        // Store the shutdown sender
        *shutdown_tx_clone.write().await = Some(shutdown_tx);

        let listener = self
            .listener
            .lock()
            .await
            .take()
            .context("HTTP server listener already consumed")?;
        info!(addr = ?addr, "HTTP server listening");
        // Start the HTTP server, any incoming requests call Host::handle and then it's routed
        // to the workload based on host header.
        let handler = self.router.clone();
        tokio::spawn(async move {
            if let Err(e) = run_http_server(
                listener,
                handler,
                workload_handles,
                &mut shutdown_rx,
                tls_acceptor,
            )
            .await
            {
                error!(err = ?e, addr = ?addr, "HTTP server error");
            }
        });

        let protocol = if self.tls_acceptor.is_some() {
            "HTTPS"
        } else {
            "HTTP"
        };
        debug!(addr = ?addr, protocol = protocol, "HTTP server starting");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!(addr = ?self.addr, "HTTP server stopping");
        let mut shutdown_guard = self.shutdown_tx.write().await;
        if let Some(tx) = shutdown_guard.take() {
            let _ = tx.send(()).await;
        }
        Ok(())
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        self.router
            .on_workload_resolved(resolved_handle, component_id)
            .await?;
        let instance_pre = resolved_handle.instantiate_pre(component_id).await?;

        self.workload_handles.write().await.insert(
            resolved_handle.id().to_string(),
            (
                resolved_handle.clone(),
                instance_pre,
                component_id.to_string(),
            ),
        );

        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        self.router.on_workload_unbind(workload_id).await?;

        self.workload_handles.write().await.remove(workload_id);

        Ok(())
    }

    fn outgoing_request(
        &self,
        workload_id: &str,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
        self.router
            .allow_outgoing_request(workload_id, &request, &config)
            .map_err(|e| {
                wasmtime_wasi_http::HttpError::trap(anyhow::anyhow!("request not allowed: {}", e))
            })?;

        if is_grpc_request(&request) {
            Ok(send_grpc_request(request, config))
        } else {
            Ok(wasmtime_wasi_http::types::default_send_request(
                request, config,
            ))
        }
    }
}

/// HTTP server implementation that routes to workload components
async fn run_http_server<T: Router>(
    listener: TcpListener,
    handler: Arc<T>,
    workload_handles: WorkloadHandles,
    shutdown_rx: &mut mpsc::Receiver<()>,
    tls_acceptor: Option<TlsAcceptor>,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            // Handle shutdown signal
            _ = shutdown_rx.recv() => {
                info!("HTTP server received shutdown signal");
                break;
            }
            // Accept new connections
            result = listener.accept() => {
                match result {
                    Ok((client, client_addr)) => {
                        debug!(addr = ?client_addr, "new HTTP client connection");

                        let handles_clone = workload_handles.clone();
                        let tls_acceptor_clone = tls_acceptor.clone();
                        let handler_clone = handler.clone();
                        tokio::spawn(async move {
                            let service = hyper::service::service_fn(move |req| {
                                let handles = handles_clone.clone();
                                let handler = handler_clone.clone();
                                async move {
                                    let extractor = opentelemetry_http::HeaderExtractor(req.headers());
                                    let remote_context =
                                        opentelemetry::global::get_text_map_propagator(|propagator| propagator.extract(&extractor));

                                    handle_http_request(handler, req, handles).with_context(remote_context).await
                                }
                            });

                            let mut builder = auto::Builder::new(TokioExecutor::new());
                            builder
                                .http1()
                                .keep_alive(true);
                            builder
                                .http2()
                                .keep_alive_interval(Some(Duration::from_secs(20)));

                            let result = if let Some(acceptor) = tls_acceptor_clone {
                                // Handle HTTPS connection
                                match acceptor.accept(client).await {
                                    Ok(tls_stream) => {
                                        builder
                                            .serve_connection_with_upgrades(TokioIo::new(tls_stream), service)
                                            .await
                                    }
                                    Err(e) => {
                                        error!(addr = ?client_addr, err = ?e, "TLS handshake failed");
                                        return;
                                    }
                                }
                            } else {
                                // Handle HTTP/h2c connection
                                builder
                                    .serve_connection_with_upgrades(TokioIo::new(client), service)
                                    .await
                            };

                            if let Err(e) = result {
                                error!(addr = ?client_addr, err = ?e, "error serving HTTP client");
                            }
                        });
                    }
                    Err(e) => {
                        error!(err = ?e, "failed to accept HTTP connection");
                    }
                }
            }
        }
    }

    Ok(())
}

/// Build an error response with the given status code.
/// Building HTTP responses with valid status codes is infallible.
#[allow(clippy::expect_used)]
fn error_response(status: u16) -> hyper::Response<HyperOutgoingBody> {
    hyper::Response::builder()
        .status(status)
        .body(HyperOutgoingBody::default())
        .expect("building HTTP response with valid status code should never fail")
}

/// Handle individual HTTP requests by looking up workload and invoking component
#[instrument(skip_all, fields(
    http.method = %req.method(),
    http.uri = %req.uri(),
    http.host = %req.headers().get(hyper::header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("unknown"),
))]
async fn handle_http_request<T: Router>(
    handler: Arc<T>,
    req: hyper::Request<hyper::body::Incoming>,
    workload_handles: WorkloadHandles,
) -> Result<hyper::Response<HyperOutgoingBody>, hyper::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();

    let Ok(workload_id) = handler.route_incoming_request(&req) else {
        return Ok(error_response(400));
    };

    debug!(
        method = %method,
        uri = %uri,
        host = %workload_id,
        "HTTP request received"
    );

    // NOTE(lxf): Separate HTTP / GRPC handling

    // Look up workload handle for this host, with wildcard fallback
    let workload_handle = {
        let handles = workload_handles.read().await;
        debug!(host = %workload_id, "looking up workload handle for host header");
        handles.get(&workload_id).cloned()
    };

    let response = match workload_handle {
        Some((handle, instance_pre, component_id)) => {
            let req_span = tracing::span!(
                tracing::Level::INFO,
                "invoke_component_handler",
                workload.name = handle.name(),
                workload.namespace = handle.namespace(),
                workload.id = handle.id(),
            );
            match invoke_component_handler(handle, instance_pre, &component_id, req)
                .instrument(req_span)
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    error!(err = ?e, "failed to invoke component");
                    // TODO: Add in the actual error message in the response body
                    // .body(HyperOutgoingBody::new(e.to_string()))
                    error_response(500)
                }
            }
        }
        None => {
            warn!(host = %workload_id, "No workload bound to host header or wildcard '*'");
            error_response(404)
        }
    };

    Ok(response)
}

/// Invoke the component handler for the given workload
async fn invoke_component_handler(
    workload_handle: ResolvedWorkload,
    instance_pre: InstancePre<SharedCtx>,
    component_id: &str,
    req: hyper::Request<hyper::body::Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    // Create a new store for this request with plugin contexts
    let store = workload_handle.new_store(component_id).await?;

    handle_component_request(store, instance_pre, req).await
}

/// Handle a component request using WASI HTTP (copied from wash/crates/src/cli/dev.rs)
pub async fn handle_component_request(
    mut store: Store<SharedCtx>,
    pre: InstancePre<SharedCtx>,
    req: hyper::Request<hyper::body::Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let scheme = match req.uri().scheme() {
        Some(scheme) if scheme == &hyper::http::uri::Scheme::HTTP => Scheme::Http,
        Some(scheme) if scheme == &hyper::http::uri::Scheme::HTTPS => Scheme::Https,
        Some(scheme) => Scheme::Other(scheme.as_str().to_string()),
        // Fallback to HTTP if no scheme is present
        None => Scheme::Http,
    };
    let req = store.data_mut().new_incoming_request(scheme, req)?;
    let out = store.data_mut().new_response_outparam(sender)?;
    let pre = ProxyPre::new(pre).context("failed to instantiate proxy pre")?;

    // Run the http request itself in a separate task so the task can
    // optionally continue to execute beyond after the initial
    // headers/response code are sent.
    let task: JoinHandle<anyhow::Result<()>> = tokio::task::spawn(
        async move {
            // Run the http request itself by instantiating and calling the component
            let proxy = pre.instantiate_async(&mut store).await?;

            proxy
                .wasi_http_incoming_handler()
                .call_handle(&mut store, req, out)
                .await?;

            Ok(())
        }
        .in_current_span(),
    );

    match receiver.await {
        // If the client calls `response-outparam::set` then one of these
        // methods will be called.
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(e.into()),

        // Otherwise the `sender` will get dropped along with the `Store`
        // meaning that the oneshot will get disconnected
        Err(e) => {
            if let Err(task_error) = task.await {
                error!(err = ?task_error, "error receiving http response");
                Err(anyhow::anyhow!(
                    "error receiving http response: {task_error}"
                ))
            } else {
                error!(err = ?e, "error receiving http response");
                Err(anyhow::anyhow!(
                    "oneshot channel closed but no response was sent"
                ))
            }
        }
    }
}

/// Load TLS configuration from certificate and key files
/// Extracted from wash dev command for reuse in HTTP server plugin
async fn load_tls_config(
    cert_path: &Path,
    key_path: &Path,
    ca_path: Option<&Path>,
) -> anyhow::Result<ServerConfig> {
    // Load certificate chain
    let cert_data = tokio::fs::read(cert_path).await.context(format!(
        "Failed to read certificate file: {}",
        cert_path.display()
    ))?;
    let mut cert_reader = std::io::Cursor::new(cert_data);
    let cert_chain: Vec<CertificateDer<'static>> = certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .context(format!(
            "Failed to parse certificate file: {}",
            cert_path.display()
        ))?;

    ensure!(
        !cert_chain.is_empty(),
        "No certificates found in file: {}",
        cert_path.display()
    );

    // Load private key
    let key_data = tokio::fs::read(key_path).await.context(format!(
        "Failed to read private key file: {}",
        key_path.display()
    ))?;
    let mut key_reader = std::io::Cursor::new(key_data);
    let key = private_key(&mut key_reader)
        .context(format!(
            "Failed to parse private key file: {}",
            key_path.display()
        ))?
        .ok_or_else(|| anyhow::anyhow!("No private key found in file: {}", key_path.display()))?;

    // Create rustls server config
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .context("Failed to create TLS configuration")?;

    // Advertise both h2 and http/1.1 via ALPN
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    // If CA is provided, configure client certificate verification
    if let Some(ca_path) = ca_path {
        let ca_data = tokio::fs::read(ca_path)
            .await
            .context(format!("Failed to read CA file: {}", ca_path.display()))?;
        let mut ca_reader = std::io::Cursor::new(ca_data);
        let ca_certs: Vec<CertificateDer<'static>> = certs(&mut ca_reader)
            .collect::<Result<Vec<_>, _>>()
            .context(format!("Failed to parse CA file: {}", ca_path.display()))?;

        ensure!(
            !ca_certs.is_empty(),
            "No CA certificates found in file: {}",
            ca_path.display()
        );

        // Note: Client certificate verification configuration would go here
        // For now, we'll keep it simple without client cert verification
        debug!("CA certificate loaded, but client certificate verification not yet implemented");
    }

    Ok(config)
}

/// Check if a request is a gRPC request based on Content-Type header.
fn is_grpc_request(req: &hyper::Request<HyperOutgoingBody>) -> bool {
    req.headers()
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/grpc"))
}

/// Send a gRPC request over HTTP/2.
fn send_grpc_request(
    request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> HostFutureIncomingResponse {
    let handle = wasmtime_wasi::runtime::spawn(async move {
        Ok(send_grpc_request_handler(request, config).await)
    });
    HostFutureIncomingResponse::pending(handle)
}

/// Async handler that sends a gRPC request using HTTP/2.
async fn send_grpc_request_handler(
    mut request: hyper::Request<HyperOutgoingBody>,
    OutgoingRequestConfig {
        use_tls,
        connect_timeout,
        first_byte_timeout,
        between_bytes_timeout,
    }: OutgoingRequestConfig,
) -> Result<IncomingResponse, wasmtime_wasi_http::bindings::http::types::ErrorCode> {
    use tokio::net::TcpStream;
    use tokio::time::timeout;
    use wasmtime_wasi_http::bindings::http::types::ErrorCode;

    let authority = if let Some(authority) = request.uri().authority() {
        if authority.port().is_some() {
            authority.to_string()
        } else {
            let port = if use_tls { 443 } else { 80 };
            format!("{authority}:{port}")
        }
    } else {
        return Err(ErrorCode::HttpRequestUriInvalid);
    };

    let tcp_stream = timeout(connect_timeout, TcpStream::connect(&authority))
        .await
        .map_err(|_| ErrorCode::ConnectionTimeout)?
        .map_err(|_| ErrorCode::ConnectionRefused)?;

    let (mut sender, worker) = if use_tls {
        use rustls::pki_types::ServerName;

        let root_cert_store = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(root_cert_store)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];

        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let mut parts = authority.split(':');
        let host = parts.next().unwrap_or(&authority);
        let domain = ServerName::try_from(host)
            .map_err(|e| {
                tracing::warn!("dns lookup error: {e:?}");
                ErrorCode::ConnectionRefused
            })?
            .to_owned();
        let stream = connector.connect(domain, tcp_stream).await.map_err(|e| {
            tracing::warn!("tls protocol error: {e:?}");
            ErrorCode::TlsProtocolError
        })?;
        let stream = TokioIo::new(stream);

        let (sender, conn) = timeout(
            connect_timeout,
            http2::handshake(TokioExecutor::new(), stream),
        )
        .await
        .map_err(|_| ErrorCode::ConnectionTimeout)?
        .map_err(hyper_request_error)?;

        let worker = wasmtime_wasi::runtime::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("dropping error {e}");
            }
        });

        (sender, worker)
    } else {
        // h2c (HTTP/2 over cleartext)
        let stream = TokioIo::new(tcp_stream);
        let (sender, conn) = timeout(
            connect_timeout,
            http2::handshake(TokioExecutor::new(), stream),
        )
        .await
        .map_err(|_| ErrorCode::ConnectionTimeout)?
        .map_err(hyper_request_error)?;

        let worker = wasmtime_wasi::runtime::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("dropping error {e}");
            }
        });

        (sender, worker)
    };

    // Strip scheme/authority from URI for the actual HTTP/2 request
    // The URI was already validated, so rebuilding with just path+query is safe
    if let Ok(uri) = hyper::Uri::builder()
        .path_and_query(
            request
                .uri()
                .path_and_query()
                .map(|p| p.as_str())
                .unwrap_or("/"),
        )
        .build()
    {
        *request.uri_mut() = uri;
    }

    let resp = timeout(first_byte_timeout, sender.send_request(request))
        .await
        .map_err(|_| ErrorCode::ConnectionReadTimeout)?
        .map_err(hyper_request_error)?
        .map(|body| body.map_err(hyper_request_error).boxed_unsync());

    Ok(IncomingResponse {
        resp,
        worker: Some(worker),
        between_bytes_timeout,
    })
}
