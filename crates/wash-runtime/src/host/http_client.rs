//! Pooled outbound HTTP client shared by the P2 and P3 egress paths.
//!
//! This replaces wasmtime's `default_send_request` transport, which opens a
//! fresh TCP connection per outgoing request. Under load-test style traffic
//! those short-lived connections pile up in TIME_WAIT until the OS runs out of
//! ephemeral ports and `connect(2)` fails with `EADDRNOTAVAIL` — surfaced to
//! guests as the misleading `DNS error: rcode="address not available"`. A
//! keep-alive pool reuses connections instead, so concurrent and repeated
//! requests to the same authority do not exhaust ports.
//!
//! Pools are keyed per workload ([`WorkloadClients`]): components never reuse
//! each other's TCP connections, so connection-scoped server state (auth,
//! rate-limit attribution) cannot leak between them — and each client keeps
//! its own TLS session-resumption store, so session tickets never resume
//! across workloads either. Port exhaustion is still prevented because it is
//! caused by a single busy workload, which keeps reusing its own pool.
//!
//! Live connections are bounded ([`ConnectionLimits`]): each workload may hold
//! at most a fixed number of connections, and all workloads together share a
//! host-wide cap, so no workload (or crowd of workloads) can exhaust the
//! host's file descriptors by fanning out to many authorities.
//!
//! It also owns the outbound TLS trust roots. wasmtime's default transport
//! trusts only the compiled-in webpki (Mozilla) roots, with no way to reach
//! hosts behind a corporate or private CA. [`ClientTlsOptions`] builds a root
//! store from a [`TrustRoots`] base (webpki and/or the platform's native
//! store, which honours `SSL_CERT_FILE`/`SSL_CERT_DIR`) with any explicitly
//! configured PEM bundles layered on top.
//!
//! The per-connection helpers ([`connect_tcp`], [`connect_tls`], the
//! connection-worker spawners) follow wasmtime's `default_send_request` error
//! mappings and serve the gRPC egress fast path in `host::http`, which manages
//! its own HTTP/2 connections rather than going through the pool.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;
use hyper_util::client::legacy::connect::{Connected, Connection, HttpConnector};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::pki_types::{CertificateDer, pem::PemObject};
use tokio::net::TcpStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;
use tracing::{debug, warn};
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::hyper_request_error;
use wasmtime_wasi_http::p2::types::{IncomingResponse, OutgoingRequestConfig};

use crate::host::http_p3::{P3Body, P3RequestErrorFuture};

/// Error type carried by the unified request body handed to the pooled client.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Unified request body: P2 and P3 bodies are mapped into this so a single
/// connection pool serves both.
type ClientBody = UnsyncBoxBody<Bytes, BoxError>;

type PoolClient = hyper_util::client::legacy::Client<BoundedConnector, ClientBody>;

/// How long an idle pooled connection is kept before being closed. Kept at or
/// below common server/LB keep-alive windows (nginx 75s, many LBs 60s) so we
/// rarely try to reuse a connection the server has already closed.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Cap on idle connections kept per authority in each workload's pool,
/// bounding the file-descriptor cost of per-workload pools on a busy host.
///
/// The cap must sit above a workload's realistic burst concurrency: every
/// connection returned to a full pool is closed, so a cap below the burst
/// size churns exactly the sockets this pool exists to keep. Measured on the
/// cosmos-data http-concurrent-load example (10 inbound × 3 outbound
/// concurrent): a cap of 4 opened 1625 connections for 3000 requests, while
/// 32 opened 31. Idle connections above actual demand still close after
/// [`POOL_IDLE_TIMEOUT`], so the steady-state cost tracks real usage, not
/// this cap.
const POOL_MAX_IDLE_PER_HOST: usize = 32;

/// How long a workload's pooled client survives without that workload making a
/// request. Eviction drops the pool (closing its idle connections — in-flight
/// requests hold their own clone and are unaffected). Kept short and equal to
/// [`POOL_IDLE_TIMEOUT`]: workloads are typically fast-running functions, and
/// a client whose connections have all idled out anyway is just memory, so
/// there is nothing worth keeping past that window.
const WORKLOAD_CLIENT_IDLE: Duration = Duration::from_secs(60);

/// Default for [`ConnectionLimits::max_per_workload`]. Sized well above
/// [`POOL_MAX_IDLE_PER_HOST`] so a workload can still burst to several
/// authorities at once, while keeping any single workload's file-descriptor
/// footprint far from the host-wide cap.
const MAX_CONNECTIONS_PER_WORKLOAD: usize = 128;

/// Default for [`ConnectionLimits::max_total`]. Kept inside common default
/// file-descriptor soft limits (1024 on many Linux distributions) with room
/// left for ingress connections, OCI pulls, and the host's own control-plane
/// traffic.
const MAX_TOTAL_CONNECTIONS: usize = 512;

/// Bounds on live outbound connections (in-flight or idle in a pool).
///
/// A permit is held for the whole life of an established connection and
/// released when it closes (pool idle timeout, workload-client eviction, or
/// error) — so reusing a pooled connection never consumes a new permit; the
/// caps only gate opening *new* connections. When a cap is reached, a request
/// waits for a permit or for an idle pooled connection, whichever frees first
/// (hyper races the two and abandons the pending connect if reuse wins); if
/// neither arrives within [`PERMIT_WAIT`], the request fails with a connect
/// timeout.
///
/// Note that idle pooled connections pin permits until they age out
/// ([`POOL_IDLE_TIMEOUT`]), so `max_total` should be sized for the number of
/// concurrently busy workloads times their expected burst, not treated as a
/// per-request budget.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionLimits {
    /// Maximum live connections a single workload may hold, across all
    /// authorities it talks to.
    pub max_per_workload: usize,
    /// Host-wide maximum live connections across all workloads combined.
    pub max_total: usize,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            max_per_workload: MAX_CONNECTIONS_PER_WORKLOAD,
            max_total: MAX_TOTAL_CONNECTIONS,
        }
    }
}

/// How long a connect attempt waits for [`ConnectionLimits`] permits before
/// failing with a timeout.
///
/// This deadline must exist: hyper's pool spawns an already-started connect
/// to completion in the background when idle-connection reuse wins the
/// checkout race, and such an abandoned attempt parked on the semaphore would
/// otherwise camp there indefinitely — holding the pool alive (which pins the
/// very idle-connection permits it is waiting for) and grabbing freed permits
/// to open connections nobody asked for. Kept well below the request-timeout
/// defaults (600s) so saturation surfaces as a prompt, classifiable connect
/// timeout instead of a long hang.
const PERMIT_WAIT: Duration = Duration::from_secs(5);

/// Built-in roots to start from before layering on
/// [`ClientTlsOptions::extra_ca_paths`].
///
/// The default is [`Webpki`](Self::Webpki), matching wasmtime's default
/// transport: an unconfigured host behaves exactly as before this option
/// existed. Trusting the platform store (and its
/// `SSL_CERT_FILE`/`SSL_CERT_DIR` overrides) is an explicit opt-in because it
/// widens the egress trust boundary to host-environment control.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TrustRoots {
    /// Compiled-in webpki (Mozilla) roots plus the platform's native store.
    /// The native store honours `SSL_CERT_FILE`/`SSL_CERT_DIR`.
    WebpkiAndNative,
    /// Compiled-in webpki roots only — reproducible, ignores the host
    /// environment.
    #[default]
    Webpki,
    /// Platform native store only.
    Native,
    /// No built-in roots: trust exactly `extra_ca_paths` and nothing else.
    /// The common corporate-CA case of pinning a single private root.
    ExtraOnly,
}

/// Trust-root options for outbound HTTPS from components.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientTlsOptions {
    /// Built-in roots to start from.
    pub roots: TrustRoots,
    /// Additional PEM CA bundle files to trust (each file may contain one or
    /// more certificates), layered on top of `roots`. Use this to reach hosts
    /// behind a corporate or otherwise private CA.
    pub extra_ca_paths: Vec<PathBuf>,
}

impl ClientTlsOptions {
    /// Build a rustls client configuration from these options.
    ///
    /// Fails when an entry in `extra_ca_paths` cannot be read or contains no
    /// usable certificate, or when the options yield an empty trust store
    /// (e.g. [`TrustRoots::ExtraOnly`] with no bundles); problems loading
    /// individual native-store certificates are logged and skipped.
    pub fn build(&self) -> anyhow::Result<Arc<rustls::ClientConfig>> {
        crate::init_crypto();
        let mut roots = match self.roots {
            TrustRoots::WebpkiAndNative | TrustRoots::Webpki => rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.into(),
            },
            TrustRoots::Native | TrustRoots::ExtraOnly => rustls::RootCertStore::empty(),
        };

        if matches!(self.roots, TrustRoots::WebpkiAndNative | TrustRoots::Native) {
            let native = rustls_native_certs::load_native_certs();
            for err in &native.errors {
                warn!(err = %err, "failed to load a native root certificate; skipping it");
            }
            let (added, ignored) = roots.add_parsable_certificates(native.certs);
            debug!(
                added,
                ignored, "loaded native root certificates for outbound TLS"
            );
        }

        for path in &self.extra_ca_paths {
            let certs = CertificateDer::pem_file_iter(path)
                .with_context(|| format!("failed to read CA bundle {}", path.display()))?
                .collect::<Result<Vec<_>, _>>()
                .with_context(|| format!("failed to parse PEM in CA bundle {}", path.display()))?;
            let (added, ignored) = roots.add_parsable_certificates(certs);
            anyhow::ensure!(
                added > 0,
                "no usable CA certificate found in {}",
                path.display()
            );
            debug!(path = %path.display(), added, ignored, "added extra CA certificates for outbound TLS");
        }

        anyhow::ensure!(
            !roots.is_empty(),
            "outbound TLS trust store is empty: {:?} roots with {} extra CA bundle(s) yielded no certificates",
            self.roots,
            self.extra_ca_paths.len()
        );

        Ok(Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        ))
    }
}

/// Process-wide default outbound TLS configuration (webpki roots only,
/// matching wasmtime's default transport), built once on first use.
pub fn default_client_tls_config() -> Arc<rustls::ClientConfig> {
    static DEFAULT: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    DEFAULT
        .get_or_init(|| {
            // Default options (webpki roots, no extra bundles) cannot fail,
            // but fall back to webpki-only roots rather than panicking if
            // that ever changes.
            ClientTlsOptions::default().build().unwrap_or_else(|err| {
                warn!(err = %err, "failed to build default outbound TLS config; falling back to webpki roots only");
                Arc::new(
                    rustls::ClientConfig::builder()
                        .with_root_certificates(rustls::RootCertStore {
                            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
                        })
                        .with_no_client_auth(),
                )
            })
        })
        .clone()
}

/// Transport-level failure while establishing an outbound connection.
///
/// The P2 and P3 egress paths use distinct wasi:http `ErrorCode` types, so the
/// shared connection helpers report through this enum and each path converts
/// with the matching `From` impl below (both follow wasmtime's
/// `default_send_request` mappings).
#[derive(Debug)]
pub(crate) enum ConnectError {
    /// The connect timeout elapsed.
    Timeout,
    /// TCP connect failed for a non-DNS reason.
    Refused,
    /// Name resolution failed ("address not available").
    Dns,
    /// The authority's host is not usable as a TLS server name.
    InvalidDnsName,
    /// The TLS handshake failed.
    Tls,
}

impl From<ConnectError> for wasmtime_wasi_http::p2::bindings::http::types::ErrorCode {
    fn from(err: ConnectError) -> Self {
        use wasmtime_wasi_http::p2::bindings::http::types::{DnsErrorPayload, ErrorCode};
        let dns_error = |rcode: &str| {
            ErrorCode::DnsError(DnsErrorPayload {
                rcode: Some(rcode.to_string()),
                info_code: Some(0),
            })
        };
        match err {
            ConnectError::Timeout => ErrorCode::ConnectionTimeout,
            ConnectError::Refused => ErrorCode::ConnectionRefused,
            ConnectError::Dns => dns_error("address not available"),
            ConnectError::InvalidDnsName => dns_error("invalid dns name"),
            ConnectError::Tls => ErrorCode::TlsProtocolError,
        }
    }
}

impl From<ConnectError> for wasmtime_wasi_http::p3::bindings::http::types::ErrorCode {
    fn from(err: ConnectError) -> Self {
        use wasmtime_wasi_http::p3::bindings::http::types::{DnsErrorPayload, ErrorCode};
        let dns_error = |rcode: &str| {
            ErrorCode::DnsError(DnsErrorPayload {
                rcode: Some(rcode.to_string()),
                info_code: Some(0),
            })
        };
        match err {
            ConnectError::Timeout => ErrorCode::ConnectionTimeout,
            ConnectError::Refused => ErrorCode::ConnectionRefused,
            ConnectError::Dns => dns_error("address not available"),
            ConnectError::InvalidDnsName => dns_error("invalid dns name"),
            ConnectError::Tls => ErrorCode::TlsProtocolError,
        }
    }
}

/// Open a TCP connection to `authority` within `connect_timeout`, mapping
/// failures the way wasmtime's default transport does.
pub(crate) async fn connect_tcp(
    authority: &str,
    connect_timeout: Duration,
) -> Result<TcpStream, ConnectError> {
    timeout(connect_timeout, TcpStream::connect(authority))
        .await
        .map_err(|_| ConnectError::Timeout)?
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrNotAvailable => ConnectError::Dns,
            _ if e
                .to_string()
                .starts_with("failed to lookup address information") =>
            {
                ConnectError::Dns
            }
            _ => ConnectError::Refused,
        })
}

/// Run a TLS client handshake over an established TCP stream, using
/// `authority`'s host portion as the SNI server name.
pub(crate) async fn connect_tls(
    tls: Arc<rustls::ClientConfig>,
    authority: &str,
    tcp_stream: TcpStream,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, ConnectError> {
    let connector = tokio_rustls::TlsConnector::from(tls);
    let domain = tls_server_name(authority).ok_or_else(|| {
        warn!(authority = %authority, "invalid TLS server name");
        ConnectError::InvalidDnsName
    })?;
    connector.connect(domain, tcp_stream).await.map_err(|e| {
        warn!("tls protocol error: {e:?}");
        ConnectError::Tls
    })
}

/// Spawn the hyper connection driver for a P2 egress connection.
///
/// P2's `IncomingResponse::worker` is an `AbortOnDropJoinHandle<()>`, so a
/// connection error can only be logged here; body errors still reach the guest
/// through the response body stream (mirrors wasmtime's
/// `default_send_request_handler`).
pub(crate) fn spawn_p2_conn_worker<F>(conn: F) -> AbortOnDropJoinHandle<()>
where
    F: Future<Output = Result<(), hyper::Error>> + Send + 'static,
{
    wasmtime_wasi::runtime::spawn(async move {
        if let Err(e) = conn.await {
            warn!(err = %e, "dropping outbound connection error");
        }
    })
}

/// Spawn the hyper connection driver for a P3 egress connection.
///
/// The returned handle doubles as the request-error future handed back to
/// wasmtime, so a connection failure is propagated to the guest via
/// [`p3_connection_error`] rather than dropped.
pub(crate) fn spawn_p3_conn_worker<F>(
    conn: F,
) -> AbortOnDropJoinHandle<Result<(), wasmtime_wasi_http::p3::bindings::http::types::ErrorCode>>
where
    F: Future<Output = Result<(), hyper::Error>> + Send + 'static,
{
    wasmtime_wasi::runtime::spawn(async move { conn.await.map_err(p3_connection_error) })
}

/// Translate an error from a hyper connection driver into a P3 [`ErrorCode`].
///
/// wasmtime's equivalent (`ErrorCode::from_hyper_response_error`) is crate
/// private, so mirror it here: a timeout becomes `HttpResponseTimeout`, and
/// everything else falls through to the public request mapping, which already
/// recovers an `ErrorCode` carried in the error's source chain.
///
/// [`ErrorCode`]: wasmtime_wasi_http::p3::bindings::http::types::ErrorCode
pub(crate) fn p3_connection_error(
    err: hyper::Error,
) -> wasmtime_wasi_http::p3::bindings::http::types::ErrorCode {
    use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

    if err.is_timeout() {
        ErrorCode::HttpResponseTimeout
    } else {
        ErrorCode::from_hyper_request_error(err)
    }
}

/// The authority (`host:port`) for a request, defaulting the port from the
/// scheme like wasmtime's default transport does.
pub(crate) fn request_authority<B>(request: &hyper::Request<B>, use_tls: bool) -> Option<String> {
    let authority = request.uri().authority()?;
    Some(if authority.port().is_some() {
        authority.to_string()
    } else {
        let port = if use_tls { 443 } else { 80 };
        format!("{authority}:{port}")
    })
}

/// Rewrite the request URI to origin form (path + query only). The scheme and
/// authority belong on the wire only when addressing a proxy, and
/// `SendRequest::send_request` does not strip them for us.
pub(crate) fn to_origin_form<B>(request: &mut hyper::Request<B>) {
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
}

/// Parse the host portion of `authority` into a TLS server name for SNI.
fn tls_server_name(authority: &str) -> Option<rustls::pki_types::ServerName<'static>> {
    // `authority` always carries a port here (request_authority adds one), and
    // IPv6 hosts are bracketed; strip both for the server name.
    let host = authority
        .rsplit_once(':')
        .map(|(host, _port)| host)
        .unwrap_or(authority)
        .trim_start_matches('[')
        .trim_end_matches(']');
    if host.is_empty() {
        return None;
    }
    rustls::pki_types::ServerName::try_from(host)
        .ok()
        .map(|name| name.to_owned())
}

/// A pooled outbound HTTP client with configurable TLS trust roots.
///
/// Cloning is cheap and shares the underlying connection pool.
#[derive(Clone)]
pub struct PooledClient {
    client: PoolClient,
    tls: Arc<rustls::ClientConfig>,
}

impl PooledClient {
    /// Create a standalone client using the given TLS configuration for HTTPS,
    /// bounded by the default [`ConnectionLimits`] (both caps private to this
    /// client). Clients created through [`WorkloadClients`] instead share the
    /// host-wide cap.
    pub fn new(tls: Arc<rustls::ClientConfig>) -> Self {
        let limits = ConnectionLimits::default();
        Self::bounded(
            tls,
            Arc::new(Semaphore::new(limits.max_per_workload)),
            Arc::new(Semaphore::new(limits.max_total)),
        )
    }

    /// Create a client whose new connections each hold one permit from
    /// `workload_permits` (this client's own budget) and one from
    /// `global_permits` (shared host-wide) for the connection's lifetime.
    fn bounded(
        tls: Arc<rustls::ClientConfig>,
        workload_permits: Arc<Semaphore>,
        global_permits: Arc<Semaphore>,
    ) -> Self {
        crate::init_crypto();
        let mut http = HttpConnector::new();
        // The inner connector sees https URIs too; scheme handling belongs to
        // the wrapping HttpsConnector.
        http.enforce_http(false);
        http.set_nodelay(true);
        let mut tls_config = (*tls).clone();
        // A fresh per-client session store: `ClientConfig::clone` shares the
        // resumption store behind an `Arc`, and rustls resumes sessions across
        // clones, so without this every workload would share one TLS
        // session-ticket cache — letting an upstream server correlate two
        // workloads via a resumed session, against this module's isolation
        // promise.
        tls_config.resumption = rustls::client::Resumption::in_memory_sessions(256);
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls_config)
            .https_or_http()
            // HTTP/1.1 only, and deliberately so: an HTTP/1.1 pooled
            // connection is checked out exclusively for one request at a time,
            // which is what lets per-workload pools guarantee components never
            // share a socket. Enabling HTTP/2 here would multiplex concurrent
            // streams over a single TCP connection — and with any pool sharing
            // wider than per-workload, put multiple components' requests on
            // the same socket simultaneously. That materially changes the
            // isolation story; do not flip this switch casually.
            .enable_http1()
            .wrap_connector(http);
        let connector = BoundedConnector {
            inner: https,
            workload_permits,
            global_permits,
        };
        let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new())
            .pool_timer(TokioTimer::new())
            .pool_idle_timeout(POOL_IDLE_TIMEOUT)
            .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
            .build(connector);
        Self { client, tls }
    }

    /// The TLS configuration this client verifies servers against.
    pub fn tls_config(&self) -> Arc<rustls::ClientConfig> {
        self.tls.clone()
    }

    /// Send a P2 outgoing request through the pool.
    pub(crate) async fn send_request_p2(
        &self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> Result<IncomingResponse, wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> {
        let OutgoingRequestConfig {
            // The URI scheme (validated upstream against this flag) tells the
            // connector whether to wrap the stream in TLS.
            use_tls: _,
            connect_timeout,
            first_byte_timeout,
            between_bytes_timeout,
        } = config;
        let request = request.map(|body| body.map_err(|e| Box::new(e) as BoxError).boxed_unsync());
        // A pooled request has no separate connect phase (the pool may reuse a
        // live connection), so the head must arrive within the combined budget.
        let head_timeout = connect_timeout.saturating_add(first_byte_timeout);
        let resp = tokio::time::timeout(head_timeout, self.client.request(request))
            .await
            .map_err(|_| {
                wasmtime_wasi_http::p2::bindings::http::types::ErrorCode::ConnectionReadTimeout
            })?
            .map_err(|e| classify_client_error(&e).into_p2())?;
        Ok(IncomingResponse {
            resp: resp.map(|body| body.map_err(hyper_request_error).boxed_unsync()),
            // Connection lifecycle is owned by the pool; there is no
            // per-request connection task to keep alive.
            worker: None,
            between_bytes_timeout,
        })
    }

    /// Send a P3 outgoing request through the pool.
    ///
    /// The returned future reports the request-body upload outcome to the
    /// guest: `Ok(())` once the body has been fully pulled, or the body's own
    /// error if producing it failed.
    pub(crate) async fn send_request_p3(
        &self,
        request: hyper::Request<P3Body>,
        options: Option<wasmtime_wasi_http::p3::RequestOptions>,
    ) -> Result<
        (hyper::Response<P3Body>, P3RequestErrorFuture),
        wasmtime_wasi_http::p3::bindings::http::types::ErrorCode,
    > {
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

        let (upload_tx, upload_rx) = tokio::sync::oneshot::channel::<Result<(), ErrorCode>>();
        let (parts, body) = request.into_parts();
        let body = UploadProbe {
            inner: body,
            done: Some(upload_tx),
        }
        .map_err(|e| Box::new(e) as BoxError)
        .boxed_unsync();
        let request = hyper::Request::from_parts(parts, body);

        let head_timeout = connect_timeout.saturating_add(first_byte_timeout);
        let resp = tokio::time::timeout(head_timeout, self.client.request(request))
            .await
            .map_err(|_| ErrorCode::ConnectionReadTimeout)?
            .map_err(|e| classify_client_error(&e).into_p3())?;

        let resp = resp.map(|body| {
            crate::host::http::TimedBody::new(body, between_bytes_timeout).boxed_unsync()
        });

        let io: P3RequestErrorFuture = Box::new(async move {
            match upload_rx.await {
                Ok(result) => result,
                // The body was dropped before completing — e.g. the server
                // responded without draining the upload. Not a guest-visible
                // failure.
                Err(_) => Ok(()),
            }
        });
        Ok((resp, io))
    }
}

/// Per-workload pooled clients sharing one TLS configuration and one
/// host-wide connection budget.
///
/// Each workload gets its own [`PooledClient`] (created lazily on first
/// request, evicted after [`WORKLOAD_CLIENT_IDLE`] without use), so a
/// workload reuses its own keep-alive connections but components never share
/// a TCP connection with each other. Every client draws new connections from
/// its own per-workload budget and from the shared host-wide budget (see
/// [`ConnectionLimits`]).
///
/// Cloning is cheap and shares the underlying client cache.
#[derive(Clone)]
pub struct WorkloadClients {
    tls: Arc<rustls::ClientConfig>,
    limits: ConnectionLimits,
    /// Host-wide budget shared by every workload's client; per-workload
    /// budgets are created fresh per client in [`Self::client`].
    global_permits: Arc<Semaphore>,
    clients: moka::sync::Cache<String, PooledClient>,
}

impl WorkloadClients {
    /// Create a per-workload client cache using the given TLS configuration
    /// for HTTPS and the default [`ConnectionLimits`].
    pub fn new(tls: Arc<rustls::ClientConfig>) -> Self {
        Self::with_limits(tls, ConnectionLimits::default())
    }

    /// Create a per-workload client cache with explicit connection bounds.
    pub fn with_limits(tls: Arc<rustls::ClientConfig>, limits: ConnectionLimits) -> Self {
        Self {
            tls,
            limits,
            global_permits: Arc::new(Semaphore::new(limits.max_total)),
            clients: moka::sync::Cache::builder()
                .time_to_idle(WORKLOAD_CLIENT_IDLE)
                .build(),
        }
    }

    /// The pooled client for `workload_id`, created on first use.
    pub fn client(&self, workload_id: &str) -> PooledClient {
        self.clients.get_with_by_ref(workload_id, || {
            PooledClient::bounded(
                self.tls.clone(),
                Arc::new(Semaphore::new(self.limits.max_per_workload)),
                self.global_permits.clone(),
            )
        })
    }

    /// The TLS configuration the per-workload clients verify servers against.
    pub fn tls_config(&self) -> Arc<rustls::ClientConfig> {
        self.tls.clone()
    }
}

/// Connector that gates every *new* connection on a per-workload and a
/// host-wide semaphore (see [`ConnectionLimits`]). Reusing an idle pooled
/// connection bypasses the connector entirely, so it needs no permit; hyper's
/// pool checkout races this connector against idle-connection reuse and drops
/// the pending connect (cancelling the permit acquisition) if reuse wins, so
/// waiting here never starves a request that a freed connection could serve.
#[derive(Clone)]
struct BoundedConnector {
    inner: hyper_rustls::HttpsConnector<HttpConnector>,
    workload_permits: Arc<Semaphore>,
    global_permits: Arc<Semaphore>,
}

impl tower_service::Service<hyper::Uri> for BoundedConnector {
    type Response = PermittedStream;
    type Error = BoxError;
    type Future = std::pin::Pin<Box<dyn Future<Output = Result<PermittedStream, BoxError>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        tower_service::Service::poll_ready(&mut self.inner, cx)
    }

    fn call(&mut self, uri: hyper::Uri) -> Self::Future {
        // Move out the connector we polled ready and leave a fresh clone
        // behind (the usual tower clone-and-swap).
        let mut inner = self.inner.clone();
        std::mem::swap(&mut self.inner, &mut inner);
        let workload_permits = self.workload_permits.clone();
        let global_permits = self.global_permits.clone();
        Box::pin(async move {
            // Acquire order (workload, then global) is fixed everywhere, and
            // waiters hold no resource another waiter needs, so waiting on
            // both cannot deadlock. `acquire_owned` only errors when the
            // semaphore is closed, which never happens.
            let acquire = async {
                let workload = workload_permits
                    .acquire_owned()
                    .await
                    .map_err(|_| std::io::Error::other("outbound connection limiter closed"))?;
                let global = global_permits
                    .acquire_owned()
                    .await
                    .map_err(|_| std::io::Error::other("outbound connection limiter closed"))?;
                Ok::<_, std::io::Error>((workload, global))
            };
            let permits = tokio::time::timeout(PERMIT_WAIT, acquire)
                .await
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "outbound connection limit reached (per-workload or host-wide)",
                    )
                })??;
            let stream = tower_service::Service::call(&mut inner, uri).await?;
            Ok(PermittedStream {
                inner: stream,
                _permits: permits,
            })
        })
    }
}

/// A connection stream carrying its [`ConnectionLimits`] permits; dropping
/// the stream (connection close) releases them.
struct PermittedStream {
    inner: hyper_rustls::MaybeHttpsStream<TokioIo<TcpStream>>,
    _permits: (OwnedSemaphorePermit, OwnedSemaphorePermit),
}

impl hyper::rt::Read for PermittedStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for PermittedStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
}

impl Connection for PermittedStream {
    fn connected(&self) -> Connected {
        self.inner.connected()
    }
}

/// Request body wrapper that reports the upload outcome over a oneshot once
/// the body has been fully pulled (or fails).
struct UploadProbe {
    inner: P3Body,
    done: Option<
        tokio::sync::oneshot::Sender<
            Result<(), wasmtime_wasi_http::p3::bindings::http::types::ErrorCode>,
        >,
    >,
}

impl hyper::body::Body for UploadProbe {
    type Data = Bytes;
    type Error = wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        use std::task::Poll;
        match std::pin::Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(None) => {
                if let Some(done) = self.done.take() {
                    let _ = done.send(Ok(()));
                }
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(err))) => {
                if let Some(done) = self.done.take() {
                    let _ = done.send(Err(err.clone()));
                }
                Poll::Ready(Some(Err(err)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

/// Protocol-agnostic classification of a pooled-client send error, mapped to
/// the P2/P3 `ErrorCode` variants below.
enum SendError {
    /// Name resolution failed.
    Dns,
    ConnectionTimeout,
    ConnectionRefused,
    TlsProtocol,
    /// The guest-provided request body failed while being uploaded.
    P2Body(wasmtime_wasi_http::p2::bindings::http::types::ErrorCode),
    P3Body(wasmtime_wasi_http::p3::bindings::http::types::ErrorCode),
    Protocol(String),
}

impl SendError {
    fn into_p2(self) -> wasmtime_wasi_http::p2::bindings::http::types::ErrorCode {
        use wasmtime_wasi_http::p2::bindings::http::types::{DnsErrorPayload, ErrorCode};
        match self {
            SendError::Dns => ErrorCode::DnsError(DnsErrorPayload {
                rcode: Some("address not available".to_string()),
                info_code: Some(0),
            }),
            SendError::ConnectionTimeout => ErrorCode::ConnectionTimeout,
            SendError::ConnectionRefused => ErrorCode::ConnectionRefused,
            SendError::TlsProtocol => ErrorCode::TlsProtocolError,
            SendError::P2Body(code) => code,
            SendError::P3Body(_) => ErrorCode::HttpProtocolError,
            SendError::Protocol(msg) => {
                warn!(err = %msg, "outbound HTTP protocol error");
                ErrorCode::HttpProtocolError
            }
        }
    }

    fn into_p3(self) -> wasmtime_wasi_http::p3::bindings::http::types::ErrorCode {
        use wasmtime_wasi_http::p3::bindings::http::types::{DnsErrorPayload, ErrorCode};
        match self {
            SendError::Dns => ErrorCode::DnsError(DnsErrorPayload {
                rcode: Some("address not available".to_string()),
                info_code: Some(0),
            }),
            SendError::ConnectionTimeout => ErrorCode::ConnectionTimeout,
            SendError::ConnectionRefused => ErrorCode::ConnectionRefused,
            SendError::TlsProtocol => ErrorCode::TlsProtocolError,
            SendError::P3Body(code) => code,
            SendError::P2Body(_) => ErrorCode::HttpProtocolError,
            SendError::Protocol(msg) => {
                warn!(err = %msg, "outbound HTTP protocol error");
                ErrorCode::HttpProtocolError
            }
        }
    }
}

/// Drill into an `io::Error`, returning the most specific nested error kind
/// and whether a rustls error (TLS failure) is wrapped inside.
///
/// `io::Error::source()` skips the wrapped error itself (it returns the
/// *wrapped error's* source), so nested `io::Error` layers — hyper-rustls
/// wraps tokio-rustls' error, which wraps the rustls error — are only
/// reachable via `get_ref()`.
fn unwrap_io_error(io: &std::io::Error) -> (std::io::ErrorKind, bool) {
    fn as_dyn<'a>(
        e: &'a (dyn std::error::Error + Send + Sync + 'static),
    ) -> &'a (dyn std::error::Error + Send + Sync + 'static) {
        e
    }
    let mut kind = io.kind();
    let mut cur = io.get_ref().map(as_dyn);
    while let Some(e) = cur {
        if e.downcast_ref::<rustls::Error>().is_some() {
            return (kind, true);
        }
        match e.downcast_ref::<std::io::Error>() {
            Some(inner) => {
                if inner.kind() != std::io::ErrorKind::Other {
                    kind = inner.kind();
                }
                cur = inner.get_ref().map(as_dyn);
            }
            None => break,
        }
    }
    (kind, false)
}

/// Walk an error's source chain looking for a `T`.
fn find_in_chain<'a, T: std::error::Error + 'static>(
    err: &'a (dyn std::error::Error + 'static),
) -> Option<&'a T> {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if let Some(found) = e.downcast_ref::<T>() {
            return Some(found);
        }
        cur = e.source();
    }
    None
}

fn classify_client_error(err: &hyper_util::client::legacy::Error) -> SendError {
    // A guest body error travels through hyper wrapped in our BoxError; give
    // it back to the guest unchanged.
    if let Some(code) =
        find_in_chain::<wasmtime_wasi_http::p2::bindings::http::types::ErrorCode>(err)
    {
        return SendError::P2Body(code.clone());
    }
    if let Some(code) =
        find_in_chain::<wasmtime_wasi_http::p3::bindings::http::types::ErrorCode>(err)
    {
        return SendError::P3Body(code.clone());
    }

    if err.is_connect() {
        if find_in_chain::<rustls::pki_types::InvalidDnsNameError>(err).is_some() {
            warn!(err = %format!("{err:?}"), "outbound TLS protocol error");
            return SendError::TlsProtocol;
        }
        if let Some(io) = find_in_chain::<std::io::Error>(err) {
            let (kind, is_tls) = unwrap_io_error(io);
            if is_tls {
                warn!(err = %format!("{err:?}"), "outbound TLS protocol error");
                return SendError::TlsProtocol;
            }
            return match kind {
                std::io::ErrorKind::AddrNotAvailable => SendError::Dns,
                std::io::ErrorKind::TimedOut => SendError::ConnectionTimeout,
                _ if io
                    .to_string()
                    .starts_with("failed to lookup address information") =>
                {
                    SendError::Dns
                }
                _ => SendError::ConnectionRefused,
            };
        }
        return SendError::ConnectionRefused;
    }

    if let Some(hyper_err) = find_in_chain::<hyper::Error>(err)
        && hyper_err.is_timeout()
    {
        return SendError::ConnectionTimeout;
    }
    SendError::Protocol(format!("{err:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tls_config_builds() {
        let config = default_client_tls_config();
        assert!(!config.crypto_provider().cipher_suites.is_empty());
    }

    /// The default must stay webpki-only: it matches wasmtime's default
    /// transport, so an unconfigured host behaves exactly as it did before
    /// trust roots became configurable. Trusting the platform store (and its
    /// `SSL_CERT_FILE`/`SSL_CERT_DIR` overrides) must remain an explicit
    /// opt-in — widening this default silently changes the egress trust
    /// boundary of every deployment.
    #[test]
    fn default_trust_roots_is_webpki_only() {
        assert_eq!(TrustRoots::default(), TrustRoots::Webpki);
    }

    #[test]
    fn extra_ca_path_must_exist() {
        let opts = ClientTlsOptions {
            extra_ca_paths: vec![PathBuf::from("/definitely/not/a/real/ca.pem")],
            ..Default::default()
        };
        assert!(opts.build().is_err());
    }

    #[test]
    fn extra_ca_path_loads_pem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.pem");
        let certified_key = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("failed to generate test certificate");
        std::fs::write(&path, certified_key.cert.pem()).unwrap();
        let opts = ClientTlsOptions {
            roots: TrustRoots::ExtraOnly,
            extra_ca_paths: vec![path],
        };
        opts.build().expect("PEM CA bundle should load");
    }

    #[test]
    fn ca_bundle_without_certificates_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.pem");
        std::fs::write(&path, "not a certificate\n").unwrap();
        let opts = ClientTlsOptions {
            roots: TrustRoots::ExtraOnly,
            extra_ca_paths: vec![path],
        };
        assert!(opts.build().is_err());
    }

    #[test]
    fn extra_only_without_bundles_is_rejected() {
        let opts = ClientTlsOptions {
            roots: TrustRoots::ExtraOnly,
            extra_ca_paths: vec![],
        };
        let err = opts.build().expect_err("an empty trust store must fail");
        assert!(err.to_string().contains("trust store is empty"), "{err}");
    }

    #[test]
    fn webpki_only_builds() {
        let opts = ClientTlsOptions {
            roots: TrustRoots::Webpki,
            extra_ca_paths: vec![],
        };
        opts.build().expect("webpki-only roots should build");
    }

    #[test]
    fn tls_server_name_handles_hosts_and_ipv6() {
        assert!(tls_server_name("example.com:443").is_some());
        assert!(tls_server_name("127.0.0.1:8443").is_some());
        assert!(tls_server_name("[::1]:8443").is_some());
        assert!(tls_server_name(":443").is_none());
    }

    /// Plain-HTTP keep-alive server that counts accepted connections and
    /// answers every request with `200 ok` after `delay`.
    async fn spawn_counting_server_with_delay(
        delay: Duration,
    ) -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let conns_clone = conns.clone();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(_) => return,
                };
                conns_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let mut pending = Vec::new();
                    loop {
                        let n = match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        pending.extend_from_slice(&buf[..n]);
                        // One response per request head; GETs carry no body.
                        while let Some(pos) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                            pending.drain(..pos + 4);
                            if !delay.is_zero() {
                                tokio::time::sleep(delay).await;
                            }
                            if stream
                                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                });
            }
        });
        (addr, conns)
    }

    async fn spawn_counting_server() -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>)
    {
        spawn_counting_server_with_delay(Duration::ZERO).await
    }

    fn p2_config(use_tls: bool) -> OutgoingRequestConfig {
        OutgoingRequestConfig {
            use_tls,
            connect_timeout: Duration::from_secs(5),
            first_byte_timeout: Duration::from_secs(5),
            between_bytes_timeout: Duration::from_secs(5),
        }
    }

    fn p2_request(uri: &str) -> hyper::Request<HyperOutgoingBody> {
        hyper::Request::builder()
            .uri(uri)
            .body(HyperOutgoingBody::default())
            .unwrap()
    }

    /// Sequential requests to the same authority must reuse one pooled
    /// connection instead of opening one per request (the per-request
    /// connections are what exhaust ephemeral ports under load and surface as
    /// `DNS error: rcode="address not available"`).
    #[tokio::test]
    async fn sequential_requests_reuse_the_pooled_connection() {
        let (addr, conns) = spawn_counting_server().await;
        let client = PooledClient::new(default_client_tls_config());

        for _ in 0..20 {
            let response = client
                .send_request_p2(p2_request(&format!("http://{addr}/")), p2_config(false))
                .await
                .expect("request should succeed");
            assert_eq!(response.resp.status(), 200);
            // Drain the body so the connection is returned to the pool.
            let _ = response.resp.into_body().collect().await;
        }

        let opened = conns.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            opened <= 2,
            "expected connection reuse across 20 sequential requests, but the server saw {opened} connections"
        );
    }

    /// Each workload gets its own pool: requests from the same workload reuse
    /// a connection, while a different workload must open its own instead of
    /// picking up the first workload's idle connection.
    #[tokio::test]
    async fn workloads_reuse_own_pool_but_never_share_connections() {
        let (addr, conns) = spawn_counting_server().await;
        let clients = WorkloadClients::new(default_client_tls_config());
        let uri = format!("http://{addr}/");

        for workload_id in ["workload-a", "workload-b"] {
            let client = clients.client(workload_id);
            for _ in 0..10 {
                let response = client
                    .send_request_p2(p2_request(&uri), p2_config(false))
                    .await
                    .expect("request should succeed");
                assert_eq!(response.resp.status(), 200);
                // Drain the body so the connection is returned to the pool.
                let _ = response.resp.into_body().collect().await;
            }
        }

        let opened = conns.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            opened >= 2,
            "workload-b must not reuse workload-a's idle connection, but the server saw only {opened} connection(s)"
        );
        assert!(
            opened <= 4,
            "expected connection reuse within each workload's pool, but the server saw {opened} connections"
        );
    }

    /// The per-workload connection bound must hold under a concurrent burst:
    /// with a cap of 2, eight concurrent requests to a slow server must be
    /// funnelled through at most two connections (waiters pick up pooled
    /// connections as they free instead of opening new ones).
    #[tokio::test]
    async fn per_workload_connection_bound_holds_under_burst() {
        let (addr, conns) = spawn_counting_server_with_delay(Duration::from_millis(50)).await;
        let clients = WorkloadClients::with_limits(
            default_client_tls_config(),
            ConnectionLimits {
                max_per_workload: 2,
                max_total: 100,
            },
        );
        let uri = format!("http://{addr}/");

        let requests = (0..8).map(|_| {
            let client = clients.client("workload-a");
            let uri = uri.clone();
            async move {
                let response = client
                    .send_request_p2(p2_request(&uri), p2_config(false))
                    .await
                    .expect("request should succeed despite waiting for a connection");
                assert_eq!(response.resp.status(), 200);
                // Drain the body so the connection is returned to the pool.
                let _ = response.resp.into_body().collect().await;
            }
        });
        futures::future::join_all(requests).await;

        let opened = conns.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            opened <= 2,
            "per-workload cap of 2 must bound connections, but the server saw {opened}"
        );
    }

    /// The host-wide bound must hold across workloads, and dropping a
    /// workload's client must release its connections' permits so other
    /// workloads can connect again.
    #[tokio::test]
    async fn global_connection_bound_holds_across_workloads() {
        let (addr, conns) = spawn_counting_server_with_delay(Duration::from_millis(50)).await;
        let clients = WorkloadClients::with_limits(
            default_client_tls_config(),
            ConnectionLimits {
                max_per_workload: 4,
                max_total: 2,
            },
        );
        let uri = format!("http://{addr}/");

        // Workload A bursts 4 concurrent requests; the global cap of 2 must
        // funnel them through at most two connections.
        let requests = (0..4).map(|_| {
            let client = clients.client("workload-a");
            let uri = uri.clone();
            async move {
                let response = client
                    .send_request_p2(p2_request(&uri), p2_config(false))
                    .await
                    .expect("request should succeed despite waiting for a connection");
                assert_eq!(response.resp.status(), 200);
                let _ = response.resp.into_body().collect().await;
            }
        });
        futures::future::join_all(requests).await;
        let opened = conns.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            opened <= 2,
            "global cap of 2 must bound connections, but the server saw {opened}"
        );

        // Drop workload A's client (cache eviction). Its pool stays alive
        // until the burst's abandoned connect attempts — spawned to
        // completion by hyper and parked on the exhausted global semaphore —
        // give up at [`PERMIT_WAIT`]; the pool then drops, closing A's idle
        // connections and releasing their permits.
        clients.clients.invalidate("workload-a");
        // moka may defer dropping the evicted value to a maintenance pass;
        // force it so the cache holds no reference either.
        clients.clients.run_pending_tasks();
        let deadline = tokio::time::Instant::now() + PERMIT_WAIT + Duration::from_secs(3);
        while clients.global_permits.available_permits() == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "global permits were never released after dropping workload A's client"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // Workload B can now connect instead of starving on permits pinned by
        // A's idle pool.
        let client_b = clients.client("workload-b");
        let response = client_b
            .send_request_p2(p2_request(&uri), p2_config(false))
            .await
            .expect("workload B should connect once A's permits are released");
        assert_eq!(response.resp.status(), 200);
    }

    /// Spawn an HTTP/1.1-over-TLS server whose certificate chains to a private
    /// CA, answering every request with `200 ok`. Returns the bound port and
    /// the CA certificate PEM.
    async fn private_ca_tls_server() -> (u16, String) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        crate::init_crypto();
        // An IP SAN, so tests can dial 127.0.0.1 directly rather than relying
        // on `localhost` resolving to the address the listener bound.
        let certified_key =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
        let ca_pem = certified_key.cert.pem();
        let cert_der = certified_key.cert.der().clone();
        let key_der =
            rustls::pki_types::PrivateKeyDer::try_from(certified_key.signing_key.serialize_der())
                .unwrap();

        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(_) => return,
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    // Failed handshakes (the untrusted-CA case) just drop.
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut buf = [0u8; 4096];
                    let mut seen = Vec::new();
                    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        match tls.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => seen.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = tls
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                        .await;
                });
            }
        });
        (port, ca_pem)
    }

    /// HTTPS to a server behind a private CA must work once that CA is added
    /// via `extra_ca_paths`, and must keep failing with a TLS error without it.
    #[tokio::test]
    async fn extra_ca_enables_https_to_private_ca_server() {
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

        let (port, ca_pem) = private_ca_tls_server().await;
        let dir = tempfile::tempdir().unwrap();
        let ca_path = dir.path().join("ca.pem");
        std::fs::write(&ca_path, ca_pem).unwrap();

        // Without the CA: the handshake must fail with a TLS error.
        let default_client = PooledClient::new(default_client_tls_config());
        let err = default_client
            .send_request_p2(
                p2_request(&format!("https://127.0.0.1:{port}/")),
                p2_config(true),
            )
            .await
            .expect_err("untrusted CA must fail");
        assert!(
            matches!(err, ErrorCode::TlsProtocolError),
            "expected TlsProtocolError, got {err:?}"
        );

        // With the CA: the same request must succeed.
        let tls = ClientTlsOptions {
            roots: TrustRoots::ExtraOnly,
            extra_ca_paths: vec![ca_path],
        }
        .build()
        .unwrap();
        let client = PooledClient::new(tls);
        let response = client
            .send_request_p2(
                p2_request(&format!("https://127.0.0.1:{port}/")),
                p2_config(true),
            )
            .await
            .expect("request with the private CA trusted should succeed");
        assert_eq!(response.resp.status(), 200);
    }
}
