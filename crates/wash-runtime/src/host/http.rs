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
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;

use crate::host::allowed_hosts::AllowedHost;
use crate::host::trigger_service::{BrokerMessage, MessagingJob};
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
use opentelemetry_semantic_conventions::attribute::{
    HTTP_REQUEST_METHOD, HTTP_RESPONSE_BODY_SIZE, HTTP_RESPONSE_STATUS_CODE, OTEL_STATUS_CODE,
    RPC_GRPC_STATUS_CODE, SERVER_ADDRESS, SERVER_PORT, URL_FULL, URL_PATH,
};
use tokio::net::{TcpListener, TcpStream};
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

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
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

/// Collect the ingress hostnames a workload's HTTP handler serves on: the
/// `host` config plus any comma-separated `host-aliases`, keeping only entries
/// that are valid RFC 1123 hostnames.
///
/// Unlike [`DynamicRouter::on_workload_resolved`], which fails a component
/// workload that declares no valid host, this is lenient: it returns whatever
/// valid hostnames exist (possibly none). Service workloads call it from their
/// startup path, where a missing host must not abort the service. It simply
/// won't be reachable via a hostname router (matching how the host-agnostic
/// `DevRouter` ignores hostnames entirely).
pub(crate) fn http_ingress_hostnames(interfaces: &[crate::wit::WitInterface]) -> Vec<String> {
    let Some(http_iface) = interfaces
        .iter()
        .find(|iface| iface.is_incoming_http_handler())
    else {
        return Vec::new();
    };

    let mut hosts = Vec::new();
    if let Some(primary) = http_iface.config.get("host")
        && is_valid_hostname(primary)
    {
        hosts.push(primary.clone());
    }
    if let Some(aliases) = http_iface.config.get("host-aliases") {
        hosts.extend(
            aliases
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && is_valid_hostname(s)),
        );
    }
    hosts
}

/// Why a request could not be routed to a workload.
#[derive(Debug)]
pub enum RouteError {
    /// Request had no `Host` header which is a genuinely malformed client
    /// request. Maps to 400.
    MissingHost,
    /// No workload is currently bound to the host.
    /// `DynamicRouter` passes the offending host header; `DevRouter` is
    /// host-agnostic and passes an empty string. Maps to 404.
    NoWorkloadForHost(String),
    /// Router is momentarily unable to read its routing table (lock
    /// contention under heavy load). Retrying should succeed. Maps to 503.
    Unavailable,
}

impl RouteError {
    /// HTTP status code for this routing failure.
    pub fn status(&self) -> u16 {
        match self {
            Self::MissingHost => 400,
            Self::NoWorkloadForHost(_) => 404,
            Self::Unavailable => 503,
        }
    }
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHost => write!(f, "request has no Host header or :authority"),
            // Empty host means the router is DevRouter and
            // simply has no workload registered so the host header is
            // irrelevant to the failure.
            Self::NoWorkloadForHost(host) if host.is_empty() => {
                write!(f, "no workload registered")
            }
            Self::NoWorkloadForHost(host) => write!(f, "no workload bound to host {host:?}"),
            Self::Unavailable => write!(f, "router is temporarily unavailable"),
        }
    }
}

impl std::error::Error for RouteError {}

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

    /// Register a workload whose long-lived service handles HTTP ingress (the
    /// service exports `wasi:http/handler`). `hostnames` are the ingress
    /// hostnames the service serves on (see [`http_ingress_hostnames`]); a
    /// hostname-keyed router registers the workload under each so requests
    /// resolve to it. Default: no-op.
    async fn on_service_http_resolved(
        &self,
        _workload_id: &str,
        _hostnames: &[String],
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Determine if the outgoing request is allowed
    fn allow_outgoing_request(
        &self,
        workload_id: &str,
        request: &hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        config: &wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        _allowed_hosts: &[AllowedHost],
    ) -> anyhow::Result<()>;

    /// Determine if a P3 outgoing request is allowed.
    fn allow_outgoing_request_p3(
        &self,
        _workload_id: &str,
        request: &hyper::Request<crate::host::http_p3::P3Body>,
        _options: Option<wasmtime_wasi_http::p3::RequestOptions>,
        allowed_hosts: &[AllowedHost],
    ) -> anyhow::Result<()> {
        check_allowed_hosts(request, allowed_hosts)
    }

    /// Pick a workload ID based on the incoming request.
    ///
    /// On failure, the returned [`RouteError`] determines the HTTP status
    /// code surfaced to the client (see [`RouteError::status`]).
    fn route_incoming_request(
        &self,
        req: &hyper::Request<hyper::body::Incoming>,
    ) -> Result<String, RouteError>;
}

/// Router that routes requests by 'Host' header, configured via WitInterface config
#[derive(Default)]
pub struct DynamicRouter {
    /// Routing tables behind a single [`ArcSwap`] so the per-request read in
    /// [`Self::select_workload`] is lock-free — a concurrent register/unbind can
    /// never make a request wait or 503. Register and unbind are copy-on-write
    /// via `rcu`; they happen on workload start/stop (rare relative to requests),
    /// so cloning the tables is cheap next to the hot read path.
    routes: ArcSwap<Routes>,
}

/// The `DynamicRouter` routing tables, swapped atomically as one unit so a
/// reader never sees the forward and reverse maps disagree.
#[derive(Default, Clone)]
struct Routes {
    /// Maps a hostname to every workload replica bound to it. A `BTreeSet` keeps
    /// membership ordered and deterministic; a request picks one replica at
    /// random (see [`DynamicRouter::select_workload`]).
    host_to_workload: HashMap<String, BTreeSet<String>>,
    /// Maps workload_id -> all hostnames (primary + aliases) registered for it,
    /// so `on_workload_unbind` can remove all entries cleanly.
    workload_to_host: HashMap<String, Vec<String>>,
}

impl DynamicRouter {
    /// Register `workload_id` under every hostname in `hosts`, updating both the
    /// forward (host -> replicas) and reverse (workload -> hosts) maps so
    /// [`Router::on_workload_unbind`] can later remove every entry cleanly.
    /// Idempotent: re-registering the same workload (e.g. a service restart)
    /// leaves the tables unchanged.
    fn register_hostnames(&self, workload_id: &str, hosts: &[String]) {
        self.routes.rcu(|cur| {
            let mut routes = (**cur).clone();
            routes
                .workload_to_host
                .insert(workload_id.to_string(), hosts.to_vec());
            for host in hosts {
                routes
                    .host_to_workload
                    .entry(host.clone())
                    .or_default()
                    .insert(workload_id.to_string());
            }
            routes
        });
    }

    /// Pick one replica bound to `host` at random so requests fan out across
    /// every replica instead of pinning to one. A per-thread PRNG
    /// ([`fastrand`]) avoids the cross-core cache-line contention a shared
    /// atomic cursor would incur under concurrent load, and spreads load just as
    /// evenly in aggregate. Split out from [`Router::route_incoming_request`] so
    /// the selection logic is unit-testable without constructing a
    /// [`hyper::body::Incoming`].
    fn select_workload(&self, host: &str) -> Result<String, RouteError> {
        // Lock-free read of a routing-table snapshot.
        let routes = self.routes.load();
        let Some(workload_set) = routes.host_to_workload.get(host) else {
            return Err(RouteError::NoWorkloadForHost(host.to_string()));
        };
        // An entry can exist but be empty; treat that as "no workload bound"
        // (same 404) and, importantly, keep the range below non-empty.
        if workload_set.is_empty() {
            return Err(RouteError::NoWorkloadForHost(host.to_string()));
        }
        let idx = fastrand::usize(..workload_set.len());
        let workload_id = workload_set
            .iter()
            .nth(idx)
            .ok_or_else(|| RouteError::NoWorkloadForHost(host.to_string()))?;
        Ok(workload_id.clone())
    }
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
        let Some(http_iface) = resolved_handle
            .host_interfaces()
            .iter()
            .find(|iface| iface.is_incoming_http_handler())
        else {
            anyhow::bail!(
                "workload did not request wasi:http/incoming-handler or wasi:http/handler interface"
            );
        };

        let primary_host = http_iface
            .config
            .get("host")
            .cloned()
            .context("no host header found")?;

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

        self.register_hostnames(resolved_handle.id(), &all_hosts);

        Ok(())
    }

    async fn on_service_http_resolved(
        &self,
        workload_id: &str,
        hostnames: &[String],
    ) -> anyhow::Result<()> {
        // A service-only workload (a p3 trigger service serving HTTP) reaches
        // routing here rather than through `on_workload_resolved`. Register its
        // hostnames exactly like a component workload so requests resolve to it.
        if hostnames.is_empty() {
            // debug, not warn: a service restart re-resolves, so a misconfigured one would spam.
            debug!(
                workload_id,
                "service has no valid ingress hostnames; not routable by the hostname router"
            );
            return Ok(());
        }
        self.register_hostnames(workload_id, hostnames);
        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        self.routes.rcu(|cur| {
            let mut routes = (**cur).clone();
            if let Some(hostnames) = routes.workload_to_host.remove(workload_id) {
                for hostname in &hostnames {
                    if let Some(workload_set) = routes.host_to_workload.get_mut(hostname) {
                        workload_set.remove(workload_id);
                        if workload_set.is_empty() {
                            routes.host_to_workload.remove(hostname);
                        }
                    }
                }
            }
            routes
        });
        Ok(())
    }

    fn allow_outgoing_request(
        &self,
        _workload_id: &str,
        request: &hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        _config: &wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        allowed_hosts: &[AllowedHost],
    ) -> anyhow::Result<()> {
        check_allowed_hosts(request, allowed_hosts)
    }

    /// Pick a workload ID based on the incoming request, spreading load at
    /// random across every replica bound to the request's `Host`.
    fn route_incoming_request(
        &self,
        req: &hyper::Request<hyper::body::Incoming>,
    ) -> Result<String, RouteError> {
        let workload_host = req
            .headers()
            .get(hyper::header::HOST)
            .and_then(|h| h.to_str().ok())
            .or_else(|| req.uri().authority().map(|a| a.as_str()))
            .ok_or(RouteError::MissingHost)?;
        // `select_workload` does a lock-free `ArcSwap` load and an in-memory
        // lookup, so it runs inline on the async worker — no `block_in_place`
        // needed (and routing works on any runtime flavor).
        self.select_workload(workload_host)
    }
}

/// Trait for custom outgoing HTTP egress. gRPC requests (P2 and P3) are
/// handled by the runtime before this trait is called.
pub trait OutgoingHandler: Send + Sync + 'static {
    /// Send a P2 outgoing HTTP request for the given `workload_id`.
    fn send_request(
        &self,
        workload_id: &str,
        request: hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::p2::HttpResult<wasmtime_wasi_http::p2::types::HostFutureIncomingResponse>;

    /// Send a P3 outgoing HTTP request for the given `workload_id`.
    ///
    /// `fut` is a future provided by the WASI runtime to communicate
    /// request-side processing errors back to the guest (for example, a
    /// connection reset that occurs while the request body is being uploaded,
    /// before or while the response arrives). Most implementations can ignore
    /// it (`_fut`). It is provided so that custom transports with out-of-band
    /// error channels can still deliver upload errors to the component after
    /// the response has been returned.
    fn send_request_p3(
        &self,
        workload_id: &str,
        request: hyper::Request<crate::host::http_p3::P3Body>,
        options: Option<wasmtime_wasi_http::p3::RequestOptions>,
        fut: crate::host::http_p3::P3RequestErrorFuture,
    ) -> crate::host::http_p3::P3SendFuture;
}

/// Default [`OutgoingHandler`] — defers to `wasmtime_wasi_http::p2::default_send_request` (P2)
/// and `wasmtime_wasi_http::p3::default_send_request` (P3).
#[derive(Default)]
pub struct DefaultOutgoingHandler;

impl OutgoingHandler for DefaultOutgoingHandler {
    fn send_request(
        &self,
        _workload_id: &str,
        request: hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::p2::HttpResult<wasmtime_wasi_http::p2::types::HostFutureIncomingResponse>
    {
        // Spawn the default handler ourselves (rather than calling
        // `default_send_request`) so the request can be wrapped in a client
        // span and the response status recorded once it arrives.
        let span = outbound_client_span(request.method(), request.uri());
        let handle = wasmtime_wasi::runtime::spawn(
            async move {
                let result =
                    wasmtime_wasi_http::p2::default_send_request_handler(request, config).await;
                match &result {
                    Ok(incoming) => record_outbound_status(incoming.resp.status()),
                    Err(_) => record_outbound_error(),
                }
                Ok(result)
            }
            .instrument(span),
        );
        Ok(HostFutureIncomingResponse::pending(handle))
    }
    fn send_request_p3(
        &self,
        _workload_id: &str,
        request: hyper::Request<crate::host::http_p3::P3Body>,
        options: Option<wasmtime_wasi_http::p3::RequestOptions>,
        _fut: crate::host::http_p3::P3RequestErrorFuture,
    ) -> crate::host::http_p3::P3SendFuture {
        Box::new(async move {
            use http_body_util::BodyExt;
            let (res, io) = wasmtime_wasi_http::p3::default_send_request(request, options).await?;
            let io: crate::host::http_p3::P3RequestErrorFuture = Box::new(io);
            Ok((res.map(BodyExt::boxed_unsync), io))
        })
    }
}

/// Development router that routes all requests to the last resolved workload
#[derive(Default)]
pub struct DevRouter {
    last_workload_id: std::sync::RwLock<Option<String>>,
}

#[async_trait::async_trait]
impl Router for DevRouter {
    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        _component_id: &str,
    ) -> anyhow::Result<()> {
        let mut lock = self
            .last_workload_id
            .write()
            .map_err(|e| anyhow::anyhow!("DevRouter write lock poisoned: {e}"))?;
        lock.replace(resolved_handle.id().to_string());
        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        let mut lock = self
            .last_workload_id
            .write()
            .map_err(|e| anyhow::anyhow!("DevRouter write lock poisoned: {e}"))?;
        if let Some(current_id) = &*lock
            && current_id == workload_id
        {
            let _ = lock.take();
        }
        Ok(())
    }

    async fn on_service_http_resolved(
        &self,
        workload_id: &str,
        _hostnames: &[String],
    ) -> anyhow::Result<()> {
        // A service-handled workload routes the same way as a component one:
        // DevRouter sends all requests to the most-recently resolved workload,
        // so it ignores hostnames.
        let mut lock = self
            .last_workload_id
            .write()
            .map_err(|e| anyhow::anyhow!("DevRouter write lock poisoned: {e}"))?;
        lock.replace(workload_id.to_string());
        Ok(())
    }

    fn allow_outgoing_request(
        &self,
        _workload_id: &str,
        request: &hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        _config: &wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        allowed_hosts: &[AllowedHost],
    ) -> anyhow::Result<()> {
        check_allowed_hosts(request, allowed_hosts)
    }

    // `allow_outgoing_request_p3` deliberately not overridden — the trait
    // default calls `check_allowed_hosts`, matching the P2 behavior above.

    /// Pick a workload ID based on the incoming request
    fn route_incoming_request(
        &self,
        _req: &hyper::Request<hyper::body::Incoming>,
    ) -> Result<String, RouteError> {
        let lock = self
            .last_workload_id
            .try_read()
            .map_err(|_| RouteError::Unavailable)?;
        match &*lock {
            Some(id) => Ok(id.clone()),
            // DevRouter is host-agnostic; signal "nothing registered" via an
            // empty host string (see RouteError::NoWorkloadForHost docs).
            None => Err(RouteError::NoWorkloadForHost(String::new())),
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

    /// Register a long-lived service instance that serves HTTP ingress: inbound
    /// requests for `workload_id` are delivered over `sender` instead of
    /// instantiating a component per request. `hostnames` are the ingress
    /// hostnames the service serves on, forwarded to the router so a
    /// hostname-keyed router can resolve requests to this workload. Default:
    /// no-op (the workload keeps the per-request path).
    async fn on_service_http_resolved(
        &self,
        _workload_id: &str,
        _hostnames: &[String],
        _sender: tokio::sync::mpsc::Sender<ServiceHttpJob>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    /// Unregister a service HTTP instance. Default: no-op.
    async fn on_service_http_unbind(&self, _workload_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    /// Register a long-lived trigger service instance that handles inbound messages:
    /// messages for `workload_id` are delivered over `sender` instead of
    /// instantiating a component per message. Default: no-op.
    async fn on_trigger_service_messaging_resolved(
        &self,
        _workload_id: &str,
        _sender: tokio::sync::mpsc::Sender<MessagingJob>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    /// Unregister a trigger service messaging instance. Default: no-op.
    async fn on_trigger_service_messaging_unbind(&self, _workload_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
    /// Deliver a message to a workload's registered messaging trigger service, returning
    /// the handler's `result<_, string>`. Default: no messaging support.
    async fn deliver_trigger_service_message(
        &self,
        _workload_id: &str,
        _msg: BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        anyhow::bail!("this host does not support trigger service messaging delivery")
    }
    /// Whether a long-lived trigger service is registered to handle messages for
    /// `workload_id` (so a host ingress can deliver to it instead of
    /// instantiating per message). Default: false.
    async fn has_trigger_service_messaging(&self, _workload_id: &str) -> bool {
        false
    }

    /// Handle an outgoing HTTP request from a workload
    fn outgoing_request(
        &self,
        workload_id: &str,
        request: hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        allowed_hosts: &[AllowedHost],
    ) -> wasmtime_wasi_http::p2::HttpResult<wasmtime_wasi_http::p2::types::HostFutureIncomingResponse>;

    /// Handle a P3 outgoing request, enforcing `allowed_hosts` policy and
    /// delegating transport to [`wasmtime_wasi_http::p3::default_send_request`].
    ///
    /// Override to apply custom egress logic (e.g. alternate transports or
    /// per-workload TLS configuration) while still honouring the allowlist via
    /// [`check_allowed_hosts`].
    fn outgoing_request_p3(
        &self,
        workload_id: &str,
        request: hyper::Request<crate::host::http_p3::P3Body>,
        options: Option<wasmtime_wasi_http::p3::RequestOptions>,
        // Response-side body-error sink: unused here because hyper's response
        // body already reports body errors through its `Stream` impl.
        _fut: crate::host::http_p3::P3RequestErrorFuture,
        allowed_hosts: &[AllowedHost],
    ) -> crate::host::http_p3::P3SendFuture {
        if let Err(e) = check_allowed_hosts(&request, allowed_hosts) {
            use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;
            warn!(workload_id = %workload_id, err = %e, "outgoing request denied by allowed_hosts policy");
            return Box::new(async move {
                Err(wasmtime_wasi::TrappableError::from(
                    ErrorCode::HttpRequestDenied,
                ))
            });
        }
        Box::new(async move {
            let (res, io) = wasmtime_wasi_http::p3::default_send_request(request, options).await?;
            let io: crate::host::http_p3::P3RequestErrorFuture = Box::new(io);
            Ok((res.map(BodyExt::boxed_unsync), io))
        })
    }
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
        _allowed_hosts: &[AllowedHost],
    ) -> wasmtime_wasi_http::p2::HttpResult<wasmtime_wasi_http::p2::types::HostFutureIncomingResponse>
    {
        Err(wasmtime_wasi_http::p2::HttpError::trap(
            wasmtime::format_err!("http client not available"),
        ))
    }

    fn outgoing_request_p3(
        &self,
        _workload_id: &str,
        _request: hyper::Request<crate::host::http_p3::P3Body>,
        _options: Option<wasmtime_wasi_http::p3::RequestOptions>,
        _fut: crate::host::http_p3::P3RequestErrorFuture,
        _allowed_hosts: &[AllowedHost],
    ) -> crate::host::http_p3::P3SendFuture {
        use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;
        Box::new(async {
            Err(wasmtime_wasi::TrappableError::from(
                ErrorCode::InternalError(Some("http client not available".to_string())),
            ))
        })
    }
}

/// A map from host header to resolved workload handles and their associated component id
pub type WorkloadHandles =
    Arc<RwLock<HashMap<String, (ResolvedWorkload, InstancePre<SharedCtx>, String)>>>;

/// An inbound HTTP request routed to a long-lived service instance, paired with
/// a oneshot for its response.
pub type ServiceHttpJob = (
    hyper::Request<hyper::body::Incoming>,
    tokio::sync::oneshot::Sender<anyhow::Result<hyper::Response<HyperOutgoingBody>>>,
);

/// A map from workload id to the channel of its HTTP-serving service instance.
/// Empty unless a workload's service opts into HTTP ingress (a p3 feature).
pub type ServiceHandlers = Arc<RwLock<HashMap<String, tokio::sync::mpsc::Sender<ServiceHttpJob>>>>;

/// A map from workload id to the channel of a trigger service's messaging handler
/// instance. Empty unless a workload's service exports a messaging handler.
pub type MessagingHandlers = Arc<RwLock<HashMap<String, tokio::sync::mpsc::Sender<MessagingJob>>>>;

/// HTTP server plugin that handles incoming HTTP requests for WebAssembly components.
///
/// This plugin implements the `wasi:http/incoming-handler` interface and routes
/// HTTP requests to appropriate WebAssembly components based on virtual hosting.
/// It supports both HTTP and HTTPS connections with optional mutual TLS.
///
/// Use [`HttpServerBuilder`] to construct an instance:
///
/// ```rust,ignore
/// let server = HttpServer::builder(router, "127.0.0.1:8080".parse()?)
///     .outgoing_handler(my_handler)
///     .tls(TlsConfig::new(cert_path, key_path))
///     .build()
///     .await?;
/// ```
pub struct HttpServer<T: Router, O: OutgoingHandler = DefaultOutgoingHandler> {
    router: Arc<T>,
    outgoing_handler: O,
    addr: SocketAddr,
    workload_handles: WorkloadHandles,
    /// Workloads whose long-lived service serves HTTP ingress directly.
    service_handlers: ServiceHandlers,
    /// Workloads whose long-lived trigger service serves messaging ingress directly.
    messaging_handlers: MessagingHandlers,
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    tls_acceptor: Option<TlsAcceptor>,
    listener: Arc<tokio::sync::Mutex<Option<TcpListener>>>,
    meters: RwLock<Meters>,
}

impl<T: Router, O: OutgoingHandler> std::fmt::Debug for HttpServer<T, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServer")
            .field("addr", &self.addr)
            .finish()
    }
}

/// TLS configuration for [`HttpServerBuilder::tls`] / [`HttpServer::new_with_tls`].
#[derive(Debug, Clone)]
pub struct TlsConfig {
    cert_path: std::path::PathBuf,
    key_path: std::path::PathBuf,
    ca_path: Option<std::path::PathBuf>,
}

impl TlsConfig {
    pub fn new(
        cert_path: impl Into<std::path::PathBuf>,
        key_path: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            cert_path: cert_path.into(),
            key_path: key_path.into(),
            ca_path: None,
        }
    }

    pub fn with_ca(mut self, ca_path: impl Into<std::path::PathBuf>) -> Self {
        self.ca_path = Some(ca_path.into());
        self
    }
}

/// Builder for [`HttpServer`].
///
/// # Required
/// - `router` and `addr` — set via [`HttpServer::builder`].
///
/// # Optional
/// - [`outgoing_handler`](Self::outgoing_handler) — defaults to [`DefaultOutgoingHandler`].
/// - [`tls`](Self::tls) — enables HTTPS.
///
/// # Example
/// ```rust,ignore
/// // Minimal — plain HTTP, default outgoing handler
/// let server = HttpServer::builder(DevRouter::default(), addr)
///     .build()
///     .await?;
///
/// // Full — HTTPS with custom egress
/// let server = HttpServer::builder(DynamicRouter::default(), addr)
///     .outgoing_handler(custom_handler)
///     .tls(TlsConfig::new(cert, key).with_ca(ca))
///     .build()
///     .await?;
/// ```
pub struct HttpServerBuilder<T: Router, O: OutgoingHandler = DefaultOutgoingHandler> {
    router: T,
    outgoing_handler: O,
    addr: SocketAddr,
    tls: Option<TlsConfig>,
}

impl<T: Router> HttpServerBuilder<T, DefaultOutgoingHandler> {
    fn new(router: T, addr: SocketAddr) -> Self {
        Self {
            router,
            outgoing_handler: DefaultOutgoingHandler,
            addr,
            tls: None,
        }
    }
}

impl<T: Router, O: OutgoingHandler> HttpServerBuilder<T, O> {
    /// Set a custom [`OutgoingHandler`], changing the builder's handler type.
    /// The same handler serves both P2 and P3 outgoing requests.
    pub fn outgoing_handler<O2: OutgoingHandler>(self, handler: O2) -> HttpServerBuilder<T, O2> {
        HttpServerBuilder {
            router: self.router,
            outgoing_handler: handler,
            addr: self.addr,
            tls: self.tls,
        }
    }

    /// Enable TLS using the given [`TlsConfig`].
    pub fn tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Bind to the address and build the [`HttpServer`].
    pub async fn build(self) -> anyhow::Result<HttpServer<T, O>> {
        crate::init_crypto();
        let tls_acceptor = match &self.tls {
            Some(tls) => {
                let config =
                    load_tls_config(&tls.cert_path, &tls.key_path, tls.ca_path.as_deref()).await?;
                Some(TlsAcceptor::from(Arc::new(config)))
            }
            None => None,
        };

        let listener = TcpListener::bind(self.addr).await?;
        let addr = listener.local_addr()?;

        Ok(HttpServer {
            router: Arc::new(self.router),
            outgoing_handler: self.outgoing_handler,
            addr,
            workload_handles: Arc::default(),
            service_handlers: Arc::default(),
            messaging_handlers: Arc::default(),
            shutdown_tx: Arc::new(RwLock::new(None)),
            tls_acceptor,
            listener: Arc::new(tokio::sync::Mutex::new(Some(listener))),
            meters: Default::default(),
        })
    }
}

impl<T: Router> HttpServer<T, DefaultOutgoingHandler> {
    /// Returns a new [`HttpServerBuilder`] with the default [`DefaultOutgoingHandler`].
    pub fn builder(router: T, addr: SocketAddr) -> HttpServerBuilder<T, DefaultOutgoingHandler> {
        HttpServerBuilder::new(router, addr)
    }

    /// Creates a new HTTP server bound to `addr` with the default outgoing handler.
    pub async fn new(router: T, addr: SocketAddr) -> anyhow::Result<Self> {
        HttpServerBuilder::new(router, addr).build().await
    }

    /// Creates a new HTTPS server with TLS and the default outgoing handler.
    pub async fn new_with_tls(router: T, addr: SocketAddr, tls: TlsConfig) -> anyhow::Result<Self> {
        HttpServerBuilder::new(router, addr).tls(tls).build().await
    }
}

impl<T: Router, O: OutgoingHandler> HttpServer<T, O> {
    /// Returns the actual bound address (useful when binding to port 0).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

#[async_trait::async_trait]
impl<T: Router, O: OutgoingHandler> HostHandler for HttpServer<T, O> {
    async fn inject_meters(&self, meters: &crate::observability::Meters) {
        *self.meters.write().await = meters.clone();
    }

    async fn start(&self) -> anyhow::Result<()> {
        let addr = self.addr;
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let shutdown_tx_clone = self.shutdown_tx.clone();
        let workload_handles = self.workload_handles.clone();
        let service_handlers = self.service_handlers.clone();
        let tls_acceptor = self.tls_acceptor.clone();

        // Store the shutdown sender
        *shutdown_tx_clone.write().await = Some(shutdown_tx);

        let listener = self
            .listener
            .lock()
            .await
            .take()
            .context("HTTP server listener already consumed")?;
        let protocol = if self.tls_acceptor.is_some() {
            "HTTPS"
        } else {
            "HTTP"
        };
        info!(addr = ?addr, protocol = protocol, "{protocol} server listening");
        // Start the HTTP server, any incoming requests call Host::handle and then it's routed
        // to the workload based on host header.
        let handler = self.router.clone();
        let fuel_meter = self.meters.read().await.fuel_consumption.clone();
        tokio::spawn(async move {
            if let Err(e) = run_http_server(
                listener,
                handler,
                workload_handles,
                service_handlers,
                &mut shutdown_rx,
                tls_acceptor,
                fuel_meter,
            )
            .await
            {
                error!(err = ?e, addr = ?addr, "HTTP server error");
            }
        });
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

        // Only components that export wasi:http are routable HTTP entrypoints.
        // Anything else stays unregistered and routes to a 404.
        if crate::engine::exports_wasi_http(instance_pre.component()) {
            self.workload_handles.write().await.insert(
                resolved_handle.id().to_string(),
                (
                    resolved_handle.clone(),
                    instance_pre,
                    component_id.to_string(),
                ),
            );
        }

        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        self.router.on_workload_unbind(workload_id).await?;

        self.workload_handles.write().await.remove(workload_id);
        self.service_handlers.write().await.remove(workload_id);
        self.messaging_handlers.write().await.remove(workload_id);

        Ok(())
    }

    async fn on_service_http_resolved(
        &self,
        workload_id: &str,
        hostnames: &[String],
        sender: tokio::sync::mpsc::Sender<ServiceHttpJob>,
    ) -> anyhow::Result<()> {
        self.router
            .on_service_http_resolved(workload_id, hostnames)
            .await?;
        // A re-resolve without an intervening unbind is expected: the trigger
        // service supervisor re-registers a fresh sender on every restart (see
        // `execute_trigger_service`) to swap in the new incarnation. Overwriting
        // is correct there — the replaced mapping belonged to the faulted
        // incarnation whose receiver is already dropped, so nothing live is
        // orphaned. Stop is the only path that unbinds.
        self.service_handlers
            .write()
            .await
            .insert(workload_id.to_string(), sender);
        Ok(())
    }

    async fn on_service_http_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        // Drop the router registration too, so a stopped service replica leaves
        // the hostname's replica set and stops being selected.
        self.router.on_workload_unbind(workload_id).await?;
        self.service_handlers.write().await.remove(workload_id);
        Ok(())
    }

    async fn on_trigger_service_messaging_resolved(
        &self,
        workload_id: &str,
        sender: tokio::sync::mpsc::Sender<MessagingJob>,
    ) -> anyhow::Result<()> {
        // As with the HTTP handler: a re-resolve without unbind is the expected
        // restart path (the supervisor swaps in the new incarnation's sender),
        // and the replaced mapping belonged to a faulted incarnation whose
        // receiver is already gone. Stop is the only path that unbinds.
        self.messaging_handlers
            .write()
            .await
            .insert(workload_id.to_string(), sender);
        Ok(())
    }

    async fn on_trigger_service_messaging_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        self.messaging_handlers.write().await.remove(workload_id);
        Ok(())
    }

    async fn deliver_trigger_service_message(
        &self,
        workload_id: &str,
        msg: BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        let sender = self
            .messaging_handlers
            .read()
            .await
            .get(workload_id)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no messaging trigger service registered for workload {workload_id}"
                )
            })?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        sender
            .send((msg, tx))
            .await
            .map_err(|_| anyhow::anyhow!("trigger service messaging instance is not running"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("trigger service dropped the message response"))
    }

    async fn has_trigger_service_messaging(&self, workload_id: &str) -> bool {
        self.messaging_handlers
            .read()
            .await
            .contains_key(workload_id)
    }

    fn outgoing_request(
        &self,
        workload_id: &str,
        request: hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
        allowed_hosts: &[AllowedHost],
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
            return Ok(send_grpc_request(request, config));
        }
        self.outgoing_handler
            .send_request(workload_id, request, config)
    }

    fn outgoing_request_p3(
        &self,
        workload_id: &str,
        request: hyper::Request<crate::host::http_p3::P3Body>,
        options: Option<wasmtime_wasi_http::p3::RequestOptions>,
        fut: crate::host::http_p3::P3RequestErrorFuture,
        allowed_hosts: &[AllowedHost],
    ) -> crate::host::http_p3::P3SendFuture {
        let span = outbound_client_span(request.method(), request.uri());
        let inner: crate::host::http_p3::P3SendFuture = if let Err(e) = self
            .router
            .allow_outgoing_request_p3(workload_id, &request, options, allowed_hosts)
        {
            warn!(workload_id = %workload_id, err = %e, "P3 outgoing request denied by allowed_hosts policy");
            use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;
            Box::new(async move {
                Err(wasmtime_wasi::TrappableError::from(
                    ErrorCode::HttpRequestDenied,
                ))
            })
        } else if is_grpc_request(&request) {
            send_grpc_request_p3(request, options)
        } else {
            self.outgoing_handler
                .send_request_p3(workload_id, request, options, fut)
        };
        // Instrument the whole send so the span is current while the response
        // is awaited; `record_outbound_status` then lands on this span.
        Box::new(
            async move {
                let result = Box::into_pin(inner).await;
                match &result {
                    Ok((resp, _)) => {
                        record_outbound_status(resp.status());
                        // No-op unless the response carries a `grpc-status` header.
                        record_grpc_status(resp.headers());
                    }
                    // Covers allowed-hosts denials and transport failures.
                    Err(_) => record_outbound_error(),
                }
                result
            }
            .instrument(span),
        )
    }
}

/// Configure a freshly accepted connection before it is served.
///
/// Disables Nagle's algorithm. Responses are written as a head segment followed
/// by body frames streamed from the guest (see [`crate::host::http_p3`]); with
/// Nagle on, the small body segment is held until the client ACKs the head, and
/// the client's delayed ACK adds a ~40ms stall to every request (write-write-read
/// deadlock). `wasmtime serve` sets `TCP_NODELAY` for the same reason.
fn prepare_accepted_conn(stream: &TcpStream) {
    if let Err(e) = stream.set_nodelay(true) {
        warn!(err = ?e, "failed to set TCP_NODELAY on accepted connection");
    }
}

/// HTTP server implementation that routes to workload components
async fn run_http_server<T: Router>(
    listener: TcpListener,
    handler: Arc<T>,
    workload_handles: WorkloadHandles,
    service_handlers: ServiceHandlers,
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

                        prepare_accepted_conn(&client);

                        let handles_clone = workload_handles.clone();
                        let service_handlers_clone = service_handlers.clone();
                        let tls_acceptor_clone = tls_acceptor.clone();
                        let handler_clone = handler.clone();
                        let fuel_meter = fuel_meter.clone();
                        tokio::spawn(async move {
                            let service = hyper::service::service_fn(move |req| {
                                let handles = handles_clone.clone();
                                let service_handlers = service_handlers_clone.clone();
                                let handler = handler_clone.clone();
                                let fuel_meter = fuel_meter.clone();
                                async move {
                                    let extractor = opentelemetry_http::HeaderExtractor(req.headers());
                                    let remote_context =
                                        opentelemetry::global::get_text_map_propagator(|propagator| propagator.extract(&extractor));

                                    handle_http_request(handler, req, handles, service_handlers, fuel_meter).with_context(remote_context).await
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
///
/// HTTP request attributes are emitted under both the current-stable OTel HTTP
/// semconv names (`http.request.method`, `url.path`, `server.address`,
/// `server.port`) and the legacy names (`http.method`, `http.uri`, `http.host`)
/// so dashboards built against either convention resolve. `server.address` holds
/// the host without its port; the port is recorded separately as `server.port`
/// when the `Host` header carries one. In addition:
/// - `http.response.status_code` is recorded before returning so span-metrics
///   collectors can break down requests by 2xx/4xx/5xx.
/// - `otel.status_code` is set to `ERROR` for 5xx; 4xx stays UNSET per semconv.
#[instrument(skip_all, fields(
    // Legacy (pre-semconv) attribute names retained so dashboards built against
    // them keep resolving.
    http.method = %req.method(),
    http.uri = %req.uri(),
    http.host = %host_header(&req),
    // Current OTel HTTP semantic conventions, referenced via the
    // `opentelemetry-semantic-conventions` constants (tracing's `{expr}` field
    // syntax resolves the constant to its attribute name at compile time).
    { HTTP_REQUEST_METHOD } = %req.method(),
    { URL_PATH } = %req.uri().path(),
    { SERVER_ADDRESS } = split_host_port(host_header(&req)).0,
    { SERVER_PORT } = tracing::field::Empty,
    { HTTP_RESPONSE_STATUS_CODE } = tracing::field::Empty,
    // Recorded once the response body has been fully streamed (see `MeteredBody`).
    { HTTP_RESPONSE_BODY_SIZE } = tracing::field::Empty,
    { OTEL_STATUS_CODE } = tracing::field::Empty,
))]
async fn handle_http_request<T: Router>(
    handler: Arc<T>,
    req: hyper::Request<hyper::body::Incoming>,
    workload_handles: WorkloadHandles,
    service_handlers: ServiceHandlers,
    fuel_meter: FuelConsumptionMeter,
) -> Result<hyper::Response<HyperOutgoingBody>, hyper::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();

    // server.port is recorded separately from server.address per the OTel HTTP
    // semconv; only set it when the Host header actually carries a port.
    if let Some(port) = split_host_port(host_header(&req)).1 {
        tracing::Span::current().record(SERVER_PORT, port);
    }

    let workload_id = match handler.route_incoming_request(&req) {
        Ok(id) => id,
        Err(e) => {
            warn!(err = %e, "failed to route incoming request");
            let resp = error_response(e.status());
            record_response_status(&resp);
            return Ok(resp);
        }
    };

    debug!(
        method = %method,
        uri = %uri,
        host = %workload_id,
        "HTTP request received"
    );

    // If this workload's long-lived service serves HTTP, deliver the request to
    // it (preserving its in-memory state) instead of the per-request path.
    let service_sender = service_handlers.read().await.get(&workload_id).cloned();
    if let Some(sender) = service_sender {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        let response = if sender.send((req, resp_tx)).await.is_err() {
            error!(host = %workload_id, "service HTTP instance is not running");
            error_response(503)
        } else {
            match resp_rx.await {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    error!(err = ?e, "service HTTP handler failed");
                    error_response(500)
                }
                Err(_) => {
                    error!("service HTTP instance dropped the response");
                    error_response(500)
                }
            }
        };
        record_response_status(&response);
        return Ok(response);
    }

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

    record_response_status(&response);
    // Carry the current span on the response body so `http.response.body.size`
    // is recorded once the HTTP server finishes streaming the body. That
    // happens after this handler future — and its `#[instrument]` span — has
    // returned, so the body wrapper keeps a span handle alive to land the
    // attribute before the span closes.
    let response =
        response.map(|body| MeteredBody::new(body, tracing::Span::current()).boxed_unsync());
    Ok(response)
}

/// Record the response's status on the current span as the OTel HTTP semconv
/// attribute `http.response.status_code`. 5xx flips `otel.status_code` to
/// `ERROR` per the HTTP semconv (4xx is a client error and stays UNSET).
fn record_response_status<B>(response: &hyper::Response<B>) {
    let status = response.status().as_u16();
    let span = tracing::Span::current();
    span.record(HTTP_RESPONSE_STATUS_CODE, status);
    if status >= 500 {
        span.record(OTEL_STATUS_CODE, "ERROR");
    }
}

/// Build a `client` span for an outbound HTTP request following the OTel HTTP
/// client semantic conventions. `http.response.status_code` and
/// `otel.status_code` are left empty for [`record_outbound_status`] to fill in
/// once the response arrives.
///
/// The span must be entered (e.g. via [`tracing::Instrument::instrument`]) for
/// the duration of the request so that status recorded on the *current* span
/// from inside the async work lands on this span.
fn outbound_client_span(method: &hyper::Method, uri: &hyper::Uri) -> tracing::Span {
    let span = tracing::info_span!(
        "outbound_http_request",
        otel.kind = "client",
        { HTTP_REQUEST_METHOD } = %method,
        { URL_FULL } = %uri,
        { SERVER_ADDRESS } = uri.host().unwrap_or_default(),
        { SERVER_PORT } = tracing::field::Empty,
        { HTTP_RESPONSE_STATUS_CODE } = tracing::field::Empty,
        { RPC_GRPC_STATUS_CODE } = tracing::field::Empty,
        { OTEL_STATUS_CODE } = tracing::field::Empty,
    );
    if let Some(port) = uri.port_u16() {
        span.record(SERVER_PORT, port);
    }
    span
}

/// Record an outbound response status on the current span. Per the HTTP client
/// semconv, both 4xx and 5xx flip `otel.status_code` to `ERROR` for client
/// spans (unlike server spans, where 4xx stays UNSET).
fn record_outbound_status(status: hyper::StatusCode) {
    let span = tracing::Span::current();
    span.record(HTTP_RESPONSE_STATUS_CODE, status.as_u16());
    if status.as_u16() >= 400 {
        span.record(OTEL_STATUS_CODE, "ERROR");
    }
}

/// Mark the current outbound span as failed when the request never produced a
/// response (allowed-hosts denial, connection failure, transport error, …).
fn record_outbound_error() {
    tracing::Span::current().record(OTEL_STATUS_CODE, "ERROR");
}

/// Record the gRPC status as `rpc.grpc.status_code` on the current span when the
/// response carries a `grpc-status` header. gRPC errors are typically
/// trailers-only responses that put the status in headers, so this captures
/// them; a status delivered in actual trailers (the streaming-success case) is
/// not read here.
fn record_grpc_status(headers: &hyper::HeaderMap) {
    if let Some(code) = headers
        .get("grpc-status")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
    {
        tracing::Span::current().record(RPC_GRPC_STATUS_CODE, code);
    }
}

/// Response-body wrapper that tallies streamed bytes and records
/// `http.response.body.size` on `span` once the body completes (ends, errors,
/// or is dropped).
///
/// The HTTP server drains the body after the handler future returns, so the
/// wrapper holds its own [`tracing::Span`] handle. That keeps the span open
/// until the body is done, ensuring the attribute is recorded before the span
/// closes.
struct MeteredBody {
    inner: HyperOutgoingBody,
    span: tracing::Span,
    bytes: u64,
    recorded: bool,
}

impl MeteredBody {
    fn new(inner: HyperOutgoingBody, span: tracing::Span) -> Self {
        Self {
            inner,
            span,
            bytes: 0,
            recorded: false,
        }
    }

    fn record(&mut self) {
        if !self.recorded {
            self.span.record(HTTP_RESPONSE_BODY_SIZE, self.bytes);
            self.recorded = true;
        }
    }
}

impl hyper::body::Body for MeteredBody {
    type Data = bytes::Bytes;
    type Error = wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        use std::task::Poll;
        match std::pin::Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    self.bytes += data.len() as u64;
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(e))) => {
                self.record();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                self.record();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for MeteredBody {
    fn drop(&mut self) {
        // Capture whatever was streamed if the body was dropped early (e.g. the
        // client disconnected mid-response).
        self.record();
    }
}

/// The request's `Host` header as a string, or `"unknown"` when absent or
/// non-UTF-8.
fn host_header<B>(req: &hyper::Request<B>) -> &str {
    req.headers()
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("unknown")
}

/// Split a `Host` header value into the OTel `server.address` (host without
/// port) and an optional `server.port`. Handles bracketed IPv6 literals such as
/// `[::1]:8080`, returning the address without brackets.
fn split_host_port(host: &str) -> (&str, Option<u16>) {
    if let Some(rest) = host.strip_prefix('[') {
        // IPv6 literal: `[addr]` or `[addr]:port`.
        if let Some((addr, after)) = rest.split_once(']') {
            let port = after.strip_prefix(':').and_then(|p| p.parse().ok());
            return (addr, port);
        }
        return (host, None);
    }
    match host.rsplit_once(':') {
        Some((addr, port)) => (addr, port.parse().ok()),
        None => (host, None),
    }
}

/// Invoke the component handler for the given workload
async fn invoke_component_handler(
    workload_handle: ResolvedWorkload,
    instance_pre: InstancePre<SharedCtx>,
    component_id: &str,
    req: hyper::Request<hyper::body::Incoming>,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let store = workload_handle.new_store(component_id).await?;

    if crate::engine::targets_wasip3_http(instance_pre.component()) {
        let resp =
            crate::host::http_p3::handle_component_request_p3(store, instance_pre, req, fuel_meter)
                .await?;
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
    let cert_chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_data)
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
    let key = PrivateKeyDer::from_pem_slice(&key_data).context(format!(
        "Failed to parse private key file: {}",
        key_path.display()
    ))?;

    // Create rustls server config
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .context("failed to create TLS configuration")?;

    // Advertise both h2 and http/1.1 via ALPN
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    // If CA is provided, configure client certificate verification
    if let Some(ca_path) = ca_path {
        let ca_data = tokio::fs::read(ca_path)
            .await
            .context(format!("failed to read CA file: {}", ca_path.display()))?;
        let ca_certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&ca_data)
            .collect::<Result<Vec<_>, _>>()
            .context(format!("failed to parse CA file: {}", ca_path.display()))?;

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

/// Checks whether an outgoing request is permitted by the `allowed_hosts`
/// policy.
///
/// **Empty list = deny all.** A workload with no entries cannot
/// reach any outbound host. Callers that want unrestricted egress must pass
/// an explicit `[AllowedHost::Any]`. The wash config layer enforces this by
/// substituting `[Any]` when `allowedHosts` is omitted from the YAML
/// (see `wash::workload::resolve_workload`), so runtime callers that come
/// through wash never see an empty list.
///
/// Otherwise the request's host (and, when the policy entry specifies them,
/// scheme and port) must satisfy at least one [`AllowedHost`] entry. See
/// [`AllowedHost::matches`] for per-variant semantics.
///
/// # Errors
///
/// Returns an error when the request has no host, when `allowed_hosts` is
/// empty, or when no policy entry matches.
pub fn check_allowed_hosts<B>(
    request: &hyper::Request<B>,
    allowed_hosts: &[AllowedHost],
) -> anyhow::Result<()> {
    let uri = request.uri();
    let request_host = uri.host().context("outgoing request has no host")?;

    if allowed_hosts.is_empty() {
        anyhow::bail!(
            "outgoing request to host '{request_host}' denied: allowed_hosts policy is empty (deny-all)"
        );
    }

    if allowed_hosts.iter().any(|entry| entry.matches(uri)) {
        return Ok(());
    }

    anyhow::bail!(
        "outgoing request to host '{request_host}' is not allowed by allowed_hosts policy"
    )
}

/// Check if a request is a gRPC request based on Content-Type header.
fn is_grpc_request<B>(req: &hyper::Request<B>) -> bool {
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
    let span = outbound_client_span(request.method(), request.uri());
    let handle = wasmtime_wasi::runtime::spawn(
        async move {
            let result = send_grpc_request_handler(request, config).await;
            match &result {
                Ok(incoming) => {
                    record_outbound_status(incoming.resp.status());
                    record_grpc_status(incoming.resp.headers());
                }
                Err(_) => record_outbound_error(),
            }
            Ok(result)
        }
        .instrument(span),
    );
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

/// P3 sibling of send_grpc_request: HTTP/2 sender for P3 outgoing gRPC.
fn send_grpc_request_p3(
    request: hyper::Request<crate::host::http_p3::P3Body>,
    options: Option<wasmtime_wasi_http::p3::RequestOptions>,
) -> crate::host::http_p3::P3SendFuture {
    Box::new(send_grpc_request_p3_handler(request, options))
}

/// Response-body wrapper enforcing a between-bytes read timeout on a streaming
/// P3 outgoing response.
///
/// `inner` is held in an `Option` so it can be dropped the instant the timeout
/// fires (or the stream ends / errors), releasing the underlying HTTP/2 stream
/// and TCP connection eagerly instead of leaving it pinned until the guest
/// drops its body handle.
struct TimedBody<B> {
    inner: Option<B>,
    interval: tokio::time::Interval,
}

impl<B> hyper::body::Body for TimedBody<B>
where
    B: hyper::body::Body<Data = bytes::Bytes, Error = hyper::Error> + Unpin,
{
    type Data = bytes::Bytes;
    type Error = wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        use std::task::{Poll, ready};
        use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

        let Some(inner) = self.inner.as_mut() else {
            return Poll::Ready(None);
        };
        match std::pin::Pin::new(inner).poll_frame(cx) {
            Poll::Ready(None) => {
                self.inner = None;
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(err))) => {
                self.inner = None;
                Poll::Ready(Some(Err(ErrorCode::from_hyper_request_error(err))))
            }
            Poll::Ready(Some(Ok(frame))) => {
                self.interval.reset();
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Pending => {
                ready!(self.interval.poll_tick(cx));
                // Release the connection before surfacing the timeout rather
                // than waiting for the guest to drop the body.
                self.inner = None;
                Poll::Ready(Some(Err(ErrorCode::ConnectionReadTimeout)))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner
            .as_ref()
            .is_none_or(hyper::body::Body::is_end_stream)
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner
            .as_ref()
            .map_or_else(hyper::body::SizeHint::default, hyper::body::Body::size_hint)
    }
}

async fn send_grpc_request_p3_handler(
    mut request: hyper::Request<crate::host::http_p3::P3Body>,
    options: Option<wasmtime_wasi_http::p3::RequestOptions>,
) -> crate::host::http_p3::P3SendResult {
    use tokio::net::TcpStream;
    use tokio::time::timeout;
    use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

    let connect_timeout = options
        .and_then(|o| o.connect_timeout)
        .unwrap_or(Duration::from_secs(600));
    let first_byte_timeout = options
        .and_then(|o| o.first_byte_timeout)
        .unwrap_or(Duration::from_secs(600));
    let between_bytes_timeout = options
        .and_then(|o| o.between_bytes_timeout)
        .unwrap_or(Duration::from_secs(600));

    let use_tls = request.uri().scheme() == Some(&hyper::http::uri::Scheme::HTTPS);

    let authority = request
        .uri()
        .authority()
        .ok_or(ErrorCode::HttpRequestUriInvalid)?;
    let authority = if authority.port().is_some() {
        authority.to_string()
    } else {
        let port = if use_tls { 443 } else { 80 };
        format!("{authority}:{port}")
    };

    let tcp_stream = timeout(connect_timeout, TcpStream::connect(&authority))
        .await
        .map_err(|_| ErrorCode::ConnectionTimeout)?
        .map_err(|_| ErrorCode::ConnectionRefused)?;

    let (mut sender, _worker) = if use_tls {
        use rustls::pki_types::ServerName;

        let root_cert_store = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(root_cert_store)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];

        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let host = authority.split(':').next().unwrap_or(&authority);
        if host.is_empty() {
            return Err(ErrorCode::HttpRequestUriInvalid.into());
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
        .map_err(ErrorCode::from_hyper_request_error)?;
        let worker = wasmtime_wasi::runtime::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("dropping error {e}");
            }
        });
        (sender, worker)
    } else {
        let stream = TokioIo::new(tcp_stream);
        let (sender, conn) = timeout(
            connect_timeout,
            http2::handshake(TokioExecutor::new(), stream),
        )
        .await
        .map_err(|_| ErrorCode::ConnectionTimeout)?
        .map_err(ErrorCode::from_hyper_request_error)?;
        let worker = wasmtime_wasi::runtime::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("dropping error {e}");
            }
        });
        (sender, worker)
    };

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
        .map_err(ErrorCode::from_hyper_request_error)?
        .map(|body| {
            let mut interval = tokio::time::interval(between_bytes_timeout);
            interval.reset();
            TimedBody {
                inner: Some(body),
                interval,
            }
            .boxed_unsync()
        });

    let io: crate::host::http_p3::P3RequestErrorFuture = Box::new(async move { Ok(()) });
    Ok((resp, io))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
    use wasmtime_wasi_http::p2::types::OutgoingRequestConfig;

    fn build_request(uri: &str) -> hyper::Request<HyperOutgoingBody> {
        hyper::Request::builder()
            .uri(uri)
            .body(HyperOutgoingBody::default())
            .unwrap()
    }

    /// Guards the P3 streaming regression fix: `run_http_server` must disable
    /// Nagle on accepted sockets. A streamed head-then-body response otherwise
    /// stalls ~40ms per request on the client's delayed ACK. The precondition
    /// asserts the socket starts with Nagle on, so this proves the helper
    /// actually flips it rather than reading a coincidental default.
    #[tokio::test]
    async fn accepted_connections_disable_nagle() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _client = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();

        assert!(
            !server.nodelay().unwrap(),
            "precondition: a freshly accepted socket should start with Nagle enabled"
        );
        prepare_accepted_conn(&server);
        assert!(
            server.nodelay().unwrap(),
            "run_http_server must set TCP_NODELAY on accepted connections"
        );
    }

    // --- check_allowed_hosts tests ---

    fn hosts(entries: &[&str]) -> Vec<AllowedHost> {
        entries.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn empty_allowed_hosts_denies_all() {
        // An empty policy means no egress. To opt into allow-all
        // the caller must pass an explicit `[AllowedHost::Any]`.
        let req = build_request("http://anything.example.com/path");
        let err = check_allowed_hosts(&req, &[]).unwrap_err();
        assert!(err.to_string().contains("deny-all"), "{}", err.to_string());
    }

    #[test]
    fn explicit_any_permits_anything() {
        let req = build_request("http://anything.example.com/path");
        assert!(check_allowed_hosts(&req, &hosts(&["*"])).is_ok());
    }

    #[test]
    fn exact_match_works() {
        let req = build_request("http://example.com/path");
        assert!(check_allowed_hosts(&req, &hosts(&["example.com"])).is_ok());
    }

    #[test]
    fn exact_match_is_case_insensitive() {
        let req = build_request("http://example.com/path");
        assert!(check_allowed_hosts(&req, &hosts(&["Example.COM"])).is_ok());
    }

    #[test]
    fn wildcard_matches_subdomain() {
        let req = build_request("http://sub.example.com/path");
        assert!(check_allowed_hosts(&req, &hosts(&["*.example.com"])).is_ok());
    }

    #[test]
    fn wildcard_does_not_match_bare_domain() {
        let req = build_request("http://example.com/path");
        assert!(check_allowed_hosts(&req, &hosts(&["*.example.com"])).is_err());
    }

    #[test]
    fn wildcard_is_case_insensitive() {
        let req = build_request("http://sub.example.com/path");
        assert!(check_allowed_hosts(&req, &hosts(&["*.Example.COM"])).is_ok());
    }

    #[test]
    fn non_matching_host_is_rejected() {
        let req = build_request("http://evil.com/path");
        let err = check_allowed_hosts(&req, &hosts(&["example.com"])).unwrap_err();
        assert!(err.to_string().contains("not allowed"));
    }

    #[test]
    fn request_with_no_host_returns_error() {
        let req = build_request("/path-only");
        let err = check_allowed_hosts(&req, &hosts(&["example.com"])).unwrap_err();
        assert!(err.to_string().contains("no host"));
    }

    #[test]
    fn star_any_matches_everything() {
        let req = build_request("http://anything.example.com/path");
        assert!(check_allowed_hosts(&req, &hosts(&["*"])).is_ok());
    }

    #[test]
    fn url_policy_pins_scheme() {
        let req = build_request("http://api.example.com/path");
        assert!(check_allowed_hosts(&req, &hosts(&["https://api.example.com"])).is_err());
    }

    // --- error_response tests ---

    #[test]
    fn error_response_returns_correct_status() {
        assert_eq!(error_response(404).status(), 404);
        assert_eq!(error_response(500).status(), 500);
    }

    // --- split_host_port tests ---

    #[test]
    fn split_host_port_bare_host() {
        assert_eq!(split_host_port("example.com"), ("example.com", None));
    }

    #[test]
    fn split_host_port_host_with_port() {
        assert_eq!(
            split_host_port("example.com:8080"),
            ("example.com", Some(8080))
        );
    }

    #[test]
    fn split_host_port_ipv4_with_port() {
        assert_eq!(split_host_port("10.1.2.80:443"), ("10.1.2.80", Some(443)));
    }

    #[test]
    fn split_host_port_ipv6_with_port() {
        assert_eq!(split_host_port("[::1]:8080"), ("::1", Some(8080)));
    }

    #[test]
    fn split_host_port_ipv6_without_port() {
        assert_eq!(split_host_port("[2001:db8::1]"), ("2001:db8::1", None));
    }

    #[test]
    fn split_host_port_invalid_port_is_dropped() {
        // Non-numeric port can't be parsed; address is still returned.
        assert_eq!(
            split_host_port("example.com:notaport"),
            ("example.com", None)
        );
    }

    // --- OutgoingHandler delegation tests ---

    fn dummy_config() -> OutgoingRequestConfig {
        OutgoingRequestConfig {
            use_tls: false,
            connect_timeout: Duration::from_secs(30),
            first_byte_timeout: Duration::from_secs(30),
            between_bytes_timeout: Duration::from_secs(30),
        }
    }

    struct SpyHandler {
        called: Arc<std::sync::atomic::AtomicBool>,
    }

    impl OutgoingHandler for SpyHandler {
        fn send_request(
            &self,
            _workload_id: &str,
            _request: hyper::Request<HyperOutgoingBody>,
            _config: OutgoingRequestConfig,
        ) -> wasmtime_wasi_http::p2::HttpResult<
            wasmtime_wasi_http::p2::types::HostFutureIncomingResponse,
        > {
            self.called.store(true, std::sync::atomic::Ordering::SeqCst);
            Err(wasmtime_wasi_http::p2::HttpError::trap(
                wasmtime::format_err!("spy: no real request"),
            ))
        }

        fn send_request_p3(
            &self,
            _workload_id: &str,
            _request: hyper::Request<crate::host::http_p3::P3Body>,
            _options: Option<wasmtime_wasi_http::p3::RequestOptions>,
            _fut: crate::host::http_p3::P3RequestErrorFuture,
        ) -> crate::host::http_p3::P3SendFuture {
            unimplemented!("spy does not implement P3")
        }
    }

    #[tokio::test]
    async fn custom_outgoing_handler_is_invoked() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server = HttpServer::builder(DevRouter::default(), "127.0.0.1:0".parse().unwrap())
            .outgoing_handler(SpyHandler {
                called: called.clone(),
            })
            .build()
            .await
            .unwrap();
        let request = build_request("http://example.com/");
        // Explicit `[Any]` policy so the deny-all-on-empty default doesn't
        // short-circuit before reaching the spy.
        let allow_any = [AllowedHost::Any];
        let _ = server.outgoing_request("test-workload", request, dummy_config(), &allow_any);
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// gRPC requests must bypass the OutgoingHandler and go directly to
    /// send_grpc_request, so the spy must NOT be called.
    #[tokio::test]
    async fn grpc_requests_bypass_outgoing_handler() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server = HttpServer::builder(DevRouter::default(), "127.0.0.1:0".parse().unwrap())
            .outgoing_handler(SpyHandler {
                called: called.clone(),
            })
            .build()
            .await
            .unwrap();
        let request = hyper::Request::builder()
            .uri("http://example.com/")
            .header(hyper::header::CONTENT_TYPE, "application/grpc")
            .body(HyperOutgoingBody::default())
            .unwrap();
        // `[Any]` lets the policy check pass so the test actually verifies
        // the gRPC dispatch path. With `&[]` (deny-all), the spy would
        // appear "not called" because policy denied — for the wrong reason.
        let allow_any = [AllowedHost::Any];
        let _ = server.outgoing_request("test-workload", request, dummy_config(), &allow_any);
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// When the between-bytes timeout fires, [`TimedBody`] must surface
    /// `ConnectionReadTimeout` *and* eagerly drop the inner body so the
    /// underlying connection is released immediately rather than lingering
    /// until the guest drops its body handle.
    #[tokio::test]
    async fn timed_body_releases_inner_on_between_bytes_timeout() {
        use http_body_util::BodyExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

        // A body that never yields a frame — guarantees the between-bytes
        // timeout fires. Its `Drop` flips a flag so the test can prove the
        // inner body (and thus its connection) is released.
        struct NeverBody {
            dropped: Arc<AtomicBool>,
        }
        impl Drop for NeverBody {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::SeqCst);
            }
        }
        impl hyper::body::Body for NeverBody {
            type Data = bytes::Bytes;
            type Error = hyper::Error;
            fn poll_frame(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>>
            {
                std::task::Poll::Pending
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let mut interval = tokio::time::interval(Duration::from_millis(10));
        interval.reset();
        let mut body = TimedBody {
            inner: Some(NeverBody {
                dropped: dropped.clone(),
            }),
            interval,
        };

        // Awaiting the next frame parks on the interval until the timeout
        // elapses, then yields the timeout error.
        match body.frame().await {
            Some(Err(ErrorCode::ConnectionReadTimeout)) => {}
            other => panic!("expected ConnectionReadTimeout, got {other:?}"),
        }
        assert!(
            body.inner.is_none(),
            "inner body should be dropped from the wrapper on timeout"
        );
        assert!(
            dropped.load(Ordering::SeqCst),
            "inner body (and its connection) should be released eagerly on timeout"
        );
    }

    /// `MeteredBody` must tally the bytes of data frames (excluding trailers)
    /// and record the total once the body completes.
    #[tokio::test]
    async fn metered_body_counts_data_frame_bytes_excluding_trailers() {
        use http_body_util::BodyExt;

        struct FramesBody {
            frames: std::collections::VecDeque<hyper::body::Frame<bytes::Bytes>>,
        }
        impl hyper::body::Body for FramesBody {
            type Data = bytes::Bytes;
            type Error = wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;
            fn poll_frame(
                mut self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>>
            {
                std::task::Poll::Ready(self.frames.pop_front().map(Ok))
            }
        }

        let frames = [
            hyper::body::Frame::data(bytes::Bytes::from_static(b"ab")),
            hyper::body::Frame::data(bytes::Bytes::from_static(b"cde")),
            // Trailers carry no body bytes and must not be counted.
            hyper::body::Frame::trailers(hyper::HeaderMap::new()),
        ]
        .into_iter()
        .collect();
        let inner: HyperOutgoingBody = FramesBody { frames }.boxed_unsync();

        let mut body = MeteredBody::new(inner, tracing::Span::none());
        while body.frame().await.is_some() {}

        assert_eq!(body.bytes, 5, "should count 2 + 3 data bytes, not trailers");
        assert!(
            body.recorded,
            "size should be recorded once the body completes"
        );
    }

    /// NullServer must deny P3 outgoing requests with an internal error,
    /// matching its P2 behaviour of returning "http client not available".
    #[tokio::test]
    async fn null_server_denies_p3_outgoing_request() {
        use crate::host::http_p3::{P3Body, P3RequestErrorFuture};
        use http_body_util::BodyExt;

        let server = NullServer::default();
        let body: P3Body = http_body_util::Empty::new()
            .map_err(|never| match never {})
            .boxed_unsync();
        let request = hyper::Request::builder()
            .uri("http://example.com/")
            .body(body)
            .unwrap();
        let fut: P3RequestErrorFuture = Box::new(async { Ok(()) });
        let result =
            Box::into_pin(server.outgoing_request_p3("test", request, None, fut, &[])).await;
        assert!(
            result.is_err(),
            "NullServer P3 outgoing request should return an error"
        );
    }

    /// A `wasi:http/incoming-handler` interface carrying `host` and, optionally,
    /// a comma-separated `host-aliases` config — mirrors what the wash config
    /// layer injects.
    fn http_iface(host: Option<&str>, aliases: Option<&str>) -> crate::wit::WitInterface {
        let mut config = HashMap::new();
        if let Some(host) = host {
            config.insert("host".to_string(), host.to_string());
        }
        if let Some(aliases) = aliases {
            config.insert("host-aliases".to_string(), aliases.to_string());
        }
        crate::wit::WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config,
            name: None,
        }
    }

    #[test]
    fn http_ingress_hostnames_collects_primary_and_valid_aliases() {
        let ifaces = vec![http_iface(Some("primary.local"), Some("a.local, b.local"))];
        assert_eq!(
            http_ingress_hostnames(&ifaces),
            vec![
                "primary.local".to_string(),
                "a.local".to_string(),
                "b.local".to_string(),
            ],
        );
    }

    #[test]
    fn http_ingress_hostnames_filters_invalid_and_empty_entries() {
        // Underscores are not valid RFC 1123 hostname chars, and leading/trailing
        // hyphens are rejected: the bad primary is dropped and only the valid
        // aliases survive.
        let ifaces = vec![http_iface(
            Some("bad_host"),
            Some("ok.local,,-nope-,also_bad,fine.local"),
        )];
        assert_eq!(
            http_ingress_hostnames(&ifaces),
            vec!["ok.local".to_string(), "fine.local".to_string()],
        );
    }

    #[test]
    fn http_ingress_hostnames_empty_without_http_interface() {
        let kv = crate::wit::WitInterface {
            namespace: "wasi".to_string(),
            package: "keyvalue".to_string(),
            interfaces: ["store".to_string()].into_iter().collect(),
            version: None,
            config: HashMap::new(),
            name: None,
        };
        assert!(http_ingress_hostnames(&[kv]).is_empty());
    }

    /// Regression guard for the "N replicas serve like one" defect: with several
    /// workloads bound to one hostname, `route_incoming_request` used to pin
    /// every request to a single arbitrary replica (`HashSet::iter().next()`).
    /// Random selection must instead spread load across all of them.
    ///
    /// Selection is a per-thread PRNG, so this asserts a distribution rather than
    /// exact counts. The band around the expected share is ~18σ wide for uniform
    /// selection over these draws, so it cannot flake, yet the old pin-to-one
    /// behavior (one replica takes everything, the rest zero) fails it outright.
    #[tokio::test]
    async fn dynamic_router_spreads_load_across_replicas() {
        let router = DynamicRouter::default();
        let replicas = ["r0", "r1", "r2", "r3"];
        for id in replicas {
            router
                .on_service_http_resolved(id, &["svc.local".to_string()])
                .await
                .unwrap();
        }

        const DRAWS: usize = 4_000;
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for _ in 0..DRAWS {
            let id = router.select_workload("svc.local").unwrap();
            *counts.entry(id).or_default() += 1;
        }

        assert_eq!(
            counts.len(),
            replicas.len(),
            "every replica should receive traffic, got {counts:?}"
        );
        let expected = DRAWS / replicas.len();
        for (id, hits) in counts {
            assert!(
                hits > expected / 2 && hits < expected * 2,
                "replica {id} got {hits}, far from the expected ~{expected} — uneven spread"
            );
        }
    }

    /// A service-only workload (defect #1) reaches routing through
    /// `on_service_http_resolved`, not `on_workload_resolved`. The router must
    /// register its hostnames so requests resolve. Previously this was a no-op
    /// and every hostname 404'd.
    #[tokio::test]
    async fn dynamic_router_registers_service_http_hostnames() {
        let router = DynamicRouter::default();
        assert!(
            matches!(
                router.select_workload("svc.local"),
                Err(RouteError::NoWorkloadForHost(_))
            ),
            "host should not resolve before the service is registered"
        );

        router
            .on_service_http_resolved(
                "svc-1",
                &["svc.local".to_string(), "svc.internal".to_string()],
            )
            .await
            .unwrap();

        assert_eq!(router.select_workload("svc.local").unwrap(), "svc-1");
        assert_eq!(router.select_workload("svc.internal").unwrap(), "svc-1");
    }

    /// A service resolving with no valid hostnames (e.g. under a host-agnostic
    /// deployment) must not register anything or error.
    #[tokio::test]
    async fn dynamic_router_service_http_empty_hostnames_is_noop() {
        let router = DynamicRouter::default();
        router.on_service_http_resolved("svc-1", &[]).await.unwrap();
        assert!(matches!(
            router.select_workload("anything.local"),
            Err(RouteError::NoWorkloadForHost(_))
        ));
    }

    /// Unbinding one replica drops it from the rotation; the hostname keeps
    /// routing to whoever remains.
    #[tokio::test]
    async fn dynamic_router_unbind_removes_replica_from_rotation() {
        let router = DynamicRouter::default();
        router
            .on_service_http_resolved("r0", &["svc.local".to_string()])
            .await
            .unwrap();
        router
            .on_service_http_resolved("r1", &["svc.local".to_string()])
            .await
            .unwrap();

        router.on_workload_unbind("r0").await.unwrap();

        for _ in 0..4 {
            assert_eq!(
                router.select_workload("svc.local").unwrap(),
                "r1",
                "only the surviving replica should be selected after unbind"
            );
        }
    }

    /// Stopping a service-only workload calls `on_service_http_unbind` but not
    /// `on_workload_unbind`. That hook must still drop the router registration,
    /// otherwise a stopped replica lingers in the hostname's replica set and
    /// requests routed onto it 404.
    #[tokio::test]
    async fn service_http_unbind_removes_router_registration() {
        let server = HttpServer::builder(DynamicRouter::default(), "127.0.0.1:0".parse().unwrap())
            .build()
            .await
            .unwrap();

        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        server
            .on_service_http_resolved("svc-1", &["svc.local".to_string()], tx)
            .await
            .unwrap();
        assert_eq!(server.router.select_workload("svc.local").unwrap(), "svc-1");

        server.on_service_http_unbind("svc-1").await.unwrap();
        assert!(
            matches!(
                server.router.select_workload("svc.local"),
                Err(RouteError::NoWorkloadForHost(_))
            ),
            "hostname must stop routing once the service unbinds"
        );
    }
}
