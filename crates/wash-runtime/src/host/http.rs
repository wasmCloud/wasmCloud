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

use crate::wit::WitInterface;
use crate::{engine::ctx::SharedCtx, observability::Meters};
use crate::{engine::workload::ResolvedWorkload, observability::FuelConsumptionMeter};
use anyhow::{Context, ensure};
use http_body_util::BodyExt;
use hyper::client::conn::http2;
use hyper_util::{
    rt::{TokioExecutor, TokioTimer},
    server::conn::auto,
};
use opentelemetry::{KeyValue, context::FutureExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, error, info, instrument, warn};
use wasmtime::Store;
use wasmtime::component::InstancePre;
use wasmtime_wasi_http::{
    io::TokioIo,
    p2::{
        WasiHttpView,
        bindings::{ProxyPre, http::types::Scheme},
        body::HyperOutgoingBody,
        hyper_request_error,
        types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
    },
};

use rustls::{ServerConfig, pki_types::CertificateDer};
use rustls_pemfile::{certs, private_key};
use tokio::sync::{RwLock, mpsc};
use tokio_rustls::TlsAcceptor;

/// Validates a hostname according to RFC 1123.
fn is_valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

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
        request: &hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        config: &wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        _allowed_hosts: &[String],
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
    /// Maps workload_id -> all hostnames (primary + aliases) registered for it.
    /// Used by on_workload_unbind to remove all entries cleanly.
    workload_to_host: tokio::sync::RwLock<HashMap<String, Vec<String>>>,
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

        let primary_host = http_iface
            .config
            .get("host")
            .cloned()
            .context("No host header found")?;

        anyhow::ensure!(
            is_valid_hostname(&primary_host),
            "primary host {primary_host:?} is not a valid RFC 1123 hostname"
        );

        // Collect primary hostname plus any DNS aliases injected by the operator.
        // Aliases are a comma-separated list of Service DNS names (e.g.
        // "my-svc,my-svc.default,my-svc.default.svc,my-svc.default.svc.cluster.local")
        // that allow cluster-internal callers to reach this workload via Service DNS.
        let mut all_hosts = vec![primary_host];
        if let Some(aliases) = http_iface.config.get("host-aliases") {
            all_hosts.extend(
                aliases
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && is_valid_hostname(s)),
            );
        }

        let workload_id = resolved_handle.id().to_string();

        {
            let mut lock = self.workload_to_host.write().await;
            lock.insert(workload_id.clone(), all_hosts.clone());
        }

        {
            let mut lock = self.host_to_workload.write().await;
            for host in &all_hosts {
                let entry = lock.entry(host.clone()).or_insert_with(HashSet::new);
                entry.insert(workload_id.clone());
            }
        }

        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        let hostnames = {
            let mut wth_lock = self.workload_to_host.write().await;
            wth_lock.remove(workload_id)
        };
        if let Some(hostnames) = hostnames {
            let mut htw_lock = self.host_to_workload.write().await;
            for hostname in &hostnames {
                if let Some(workload_set) = htw_lock.get_mut(hostname) {
                    workload_set.remove(workload_id);
                    if workload_set.is_empty() {
                        htw_lock.remove(hostname);
                    }
                }
            }
        }
        Ok(())
    }

    fn allow_outgoing_request(
        &self,
        _workload_id: &str,
        request: &hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        _config: &wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        allowed_hosts: &[String],
    ) -> anyhow::Result<()> {
        check_allowed_hosts(request, allowed_hosts)
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
        _request: &hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        _config: &wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        _allowed_hosts: &[String],
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
    /// Inject meters into the handler
    async fn inject_meters(&self, _meters: &Meters) {}
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
        request: hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        allowed_hosts: &[String],
    ) -> wasmtime_wasi_http::p2::HttpResult<wasmtime_wasi_http::p2::types::HostFutureIncomingResponse>;
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
        _request: hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        _config: wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        _allowed_hosts: &[String],
    ) -> wasmtime_wasi_http::p2::HttpResult<wasmtime_wasi_http::p2::types::HostFutureIncomingResponse>
    {
        Err(wasmtime_wasi_http::p2::HttpError::trap(
            wasmtime::format_err!("http client not available"),
        ))
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
    meters: RwLock<Meters>,
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
            meters: Default::default(),
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
            meters: Default::default(),
        })
    }
}

#[async_trait::async_trait]
impl<T: Router> HostHandler for HttpServer<T> {
    async fn inject_meters(&self, meters: &crate::observability::Meters) {
        *self.meters.write().await = meters.clone();
    }

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
        let fuel_meter = self.meters.read().await.fuel_consumption.clone();
        tokio::spawn(async move {
            if let Err(e) = run_http_server(
                listener,
                handler,
                workload_handles,
                &mut shutdown_rx,
                tls_acceptor,
                fuel_meter,
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
        request: hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        allowed_hosts: &[String],
    ) -> wasmtime_wasi_http::p2::HttpResult<wasmtime_wasi_http::p2::types::HostFutureIncomingResponse>
    {
        if let Err(e) =
            self.router
                .allow_outgoing_request(workload_id, &request, &config, allowed_hosts)
        {
            warn!(workload_id = %workload_id, err = %e, "outgoing request denied by allowed_hosts policy");
            return Err(wasmtime_wasi_http::p2::HttpError::trap(
                wasmtime_wasi_http::p2::bindings::http::types::ErrorCode::HttpRequestDenied,
            ));
        }

        if is_grpc_request(&request) {
            Ok(send_grpc_request(request, config))
        } else {
            Ok(wasmtime_wasi_http::p2::default_send_request(
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
    fuel_meter: FuelConsumptionMeter,
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
                         let fuel_meter = fuel_meter.clone();
                        tokio::spawn(async move {
                            let service = hyper::service::service_fn(move |req| {
                                let handles = handles_clone.clone();
                                let handler = handler_clone.clone();
                                 let fuel_meter = fuel_meter.clone();
                                async move {
                                    let extractor = opentelemetry_http::HeaderExtractor(req.headers());
                                    let remote_context =
                                        opentelemetry::global::get_text_map_propagator(|propagator| propagator.extract(&extractor));

                                    handle_http_request(handler, req, handles, fuel_meter).with_context(remote_context).await
                                }
                            });

                            let mut builder = auto::Builder::new(TokioExecutor::new());
                            builder
                                .http1()
                                .keep_alive(true);
                            builder
                                .http2()
                                .timer(TokioTimer::new())
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
    fuel_meter: FuelConsumptionMeter,
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
            match invoke_component_handler(handle, instance_pre, &component_id, req, fuel_meter)
                .instrument(req_span)
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    error!(err = ?e, "failed to invoke component");
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
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    // Create a new store for this request with plugin contexts
    let store = workload_handle.new_store(component_id).await?;

    // Check if this component targets WASIP3 and dispatch accordingly
    #[cfg(feature = "wasip3")]
    if crate::engine::targets_wasip3_http(instance_pre.component()) {
        let resp =
            crate::host::http_p3::handle_component_request_p3(store, instance_pre, req, fuel_meter)
                .await?;
        // Convert P3 response to a compatible HyperOutgoingBody response
        let (parts, body) = resp.into_parts();
        let body = HyperOutgoingBody::new(
            body.map_err(|e| {
                wasmtime_wasi_http::p2::bindings::http::types::ErrorCode::InternalError(Some(
                    format!("failed to convert P3 http body: {e:?}"),
                ))
            })
            .boxed_unsync(),
        );
        return Ok(hyper::Response::from_parts(parts, body));
    }

    handle_component_request(store, instance_pre, req, fuel_meter).await
}

/// Handle a component request using WASI HTTP (copied from wash/crates/src/cli/dev.rs)
pub async fn handle_component_request(
    mut store: Store<SharedCtx>,
    pre: InstancePre<SharedCtx>,
    req: hyper::Request<hyper::body::Incoming>,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let scheme = match req.uri().scheme() {
        Some(scheme) if scheme == &hyper::http::uri::Scheme::HTTP => Scheme::Http,
        Some(scheme) if scheme == &hyper::http::uri::Scheme::HTTPS => Scheme::Https,
        Some(scheme) => Scheme::Other(scheme.as_str().to_string()),
        // Fallback to HTTP if no scheme is present
        None => Scheme::Http,
    };

    let method = req.method().to_string();
    let host_header = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| h.to_string())
        .unwrap_or_default();
    let uri = req.uri().to_string();

    let req = store.data_mut().http().new_incoming_request(scheme, req)?;
    let out = store.data_mut().http().new_response_outparam(sender)?;
    let pre = ProxyPre::new(pre)
        .map_err(anyhow::Error::from)
        .context("failed to instantiate proxy pre")?;

    // Run the http request itself in a separate task so the task can
    // optionally continue to execute beyond after the initial
    // headers/response code are sent.
    let task: JoinHandle<anyhow::Result<()>> = tokio::task::spawn(
        async move {
            // Run the http request itself by instantiating and calling the component
            let proxy = pre.instantiate_async(&mut store).await?;

            fuel_meter
                .observe(
                    &[
                        KeyValue::new("plugin", "wasi-http"),
                        KeyValue::new("method", method),
                        KeyValue::new("host", host_header),
                        KeyValue::new("uri", uri),
                    ],
                    &mut store,
                    async move |store| {
                        proxy
                            .wasi_http_incoming_handler()
                            .call_handle(store, req, out)
                            .await?;

                        Ok(())
                    },
                )
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

/// Check if an outgoing request's host is permitted by the allowed_hosts list.
///
/// If `allowed_hosts` is empty, all requests are allowed.
/// Supports wildcard patterns like `*.example.com` which match any subdomain.
pub fn check_allowed_hosts<B>(
    request: &hyper::Request<B>,
    allowed_hosts: &[String],
) -> anyhow::Result<()> {
    if allowed_hosts.is_empty() {
        return Ok(());
    }

    let request_host = request
        .uri()
        .host()
        .context("outgoing request has no host")?;

    let request_host_lower = request_host.to_ascii_lowercase();
    for pattern in allowed_hosts {
        if let Some(suffix) = pattern.strip_prefix('*') {
            // Wildcard: *.example.com matches foo.example.com but not example.com
            let suffix_lower = suffix.to_ascii_lowercase();
            if let Some(prefix) = request_host_lower.strip_suffix(suffix_lower.as_str())
                && !prefix.is_empty()
            {
                return Ok(());
            }
        } else if request_host.eq_ignore_ascii_case(pattern) {
            return Ok(());
        }
    }

    anyhow::bail!(
        "outgoing request to host '{}' is not allowed by allowed_hosts policy",
        request_host
    )
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
) -> Result<IncomingResponse, wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> {
    use tokio::net::TcpStream;
    use tokio::time::timeout;
    use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

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
        if host.is_empty() {
            return Err(ErrorCode::HttpRequestUriInvalid);
        }
        let domain = ServerName::try_from(host)
            .map_err(|e| {
                tracing::warn!("invalid server name '{host}': {e:?}");
                ErrorCode::HttpRequestUriInvalid
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

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime_wasi_http::p2::body::HyperOutgoingBody;

    fn build_request(uri: &str) -> hyper::Request<HyperOutgoingBody> {
        hyper::Request::builder()
            .uri(uri)
            .body(HyperOutgoingBody::default())
            .unwrap()
    }

    // --- check_allowed_hosts tests ---

    #[test]
    fn empty_allowed_hosts_permits_any() {
        let req = build_request("http://anything.example.com/path");
        assert!(check_allowed_hosts(&req, &[]).is_ok());
    }

    #[test]
    fn exact_match_works() {
        let req = build_request("http://example.com/path");
        let hosts = vec!["example.com".to_string()];
        assert!(check_allowed_hosts(&req, &hosts).is_ok());
    }

    #[test]
    fn exact_match_is_case_insensitive() {
        let req = build_request("http://example.com/path");
        let hosts = vec!["Example.COM".to_string()];
        assert!(check_allowed_hosts(&req, &hosts).is_ok());
    }

    #[test]
    fn wildcard_matches_subdomain() {
        let req = build_request("http://sub.example.com/path");
        let hosts = vec!["*.example.com".to_string()];
        assert!(check_allowed_hosts(&req, &hosts).is_ok());
    }

    #[test]
    fn wildcard_does_not_match_bare_domain() {
        let req = build_request("http://example.com/path");
        let hosts = vec!["*.example.com".to_string()];
        assert!(check_allowed_hosts(&req, &hosts).is_err());
    }

    #[test]
    fn wildcard_is_case_insensitive() {
        let req = build_request("http://sub.example.com/path");
        let hosts = vec!["*.Example.COM".to_string()];
        assert!(check_allowed_hosts(&req, &hosts).is_ok());
    }

    #[test]
    fn non_matching_host_is_rejected() {
        let req = build_request("http://evil.com/path");
        let hosts = vec!["example.com".to_string()];
        let err = check_allowed_hosts(&req, &hosts).unwrap_err();
        assert!(err.to_string().contains("not allowed"));
    }

    #[test]
    fn request_with_no_host_returns_error() {
        let req = build_request("/path-only");
        let hosts = vec!["example.com".to_string()];
        let err = check_allowed_hosts(&req, &hosts).unwrap_err();
        assert!(err.to_string().contains("no host"));
    }

    // --- error_response tests ---

    #[test]
    fn error_response_returns_correct_status() {
        assert_eq!(error_response(404).status(), 404);
        assert_eq!(error_response(500).status(), 500);
    }
}
