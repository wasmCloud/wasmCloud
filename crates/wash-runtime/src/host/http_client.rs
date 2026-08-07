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
//! Live connections are bounded by the workload's
//! [`GuestConnectionQuota`](crate::host::quota::GuestConnectionQuota): its
//! `http` surface caps how large this pool may grow, and every surface rolls
//! up into a host-wide ceiling, so no workload (or crowd of workloads) can
//! exhaust the host's file descriptors by fanning out to many authorities.
//! The same quota bounds the workload's raw sockets and its inbound published
//! ports, so one set of numbers governs everything it holds.
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

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::combinators::UnsyncBoxBody;
use hyper_util::client::legacy::connect::{
    CaptureConnection, Connected, Connection, HttpConnector, capture_connection,
};
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

/// Floor for the idle connections kept per authority in a workload's pool,
/// and the whole cap for a workload whose concurrency the host does not know.
///
/// The cap must sit above a workload's realistic burst concurrency: every
/// connection returned to a full pool is closed, so a cap below the burst
/// size churns exactly the sockets this pool exists to keep. Measured by
/// driving a component that fans each of 10 concurrent inbound requests out
/// to 3 concurrent outbound ones, against a backend counting accepted TCP
/// connections: over 3000 outbound requests, a cap of 4 opened 1625
/// connections while a cap of 32 opened 31. Idle connections above actual
/// demand still close after [`POOL_IDLE_TIMEOUT`], so the steady-state cost
/// tracks real usage, not this cap.
const MIN_IDLE_PER_AUTHORITY: usize = 32;

/// Outbound requests one guest call is assumed to have in flight at once,
/// used to turn a workload's declared call concurrency into an idle cap.
///
/// The host cannot see a guest's fan-out — how many outbound requests one
/// inbound call makes concurrently is entirely up to the guest — so this is
/// an assumption, chosen a little above the fan-out of 3 the measurement in
/// [`MIN_IDLE_PER_AUTHORITY`] used. Guessing high costs only idle sockets
/// that [`POOL_IDLE_TIMEOUT`] reclaims and that
/// [`ConnectionLimits::max_per_workload`] already bounds; guessing low costs
/// connection churn under exactly the load this pool exists for.
const ASSUMED_OUTBOUND_FANOUT: usize = 4;

/// Idle connections to keep per authority for a workload that declared it
/// may have `call_concurrency` guest calls in flight at once.
///
/// A workload's concurrent outbound requests scale with the calls it runs at
/// once — `pool_size` × `max_concurrency` for a component keeping instances
/// warm, one otherwise — times whatever each call fans out to. Sizing the
/// idle cap off a fixed number instead would leave a component that opted
/// into instance concurrency churning connections: its burst outgrows the
/// cap, and every connection returned to a full pool is closed.
///
/// The floor applies first and [`ConnectionLimits::max_per_workload`] last,
/// because the budget is the real bound on the workload's live connections —
/// permits, not this cap, are what stop it exhausting file descriptors, and a
/// workload can never hold more idle than its whole budget. This only decides
/// how much of that budget one authority may hold *idle*. A budget under the
/// floor is therefore honoured rather than raised to it.
fn idle_per_authority(call_concurrency: usize, max_http: usize) -> usize {
    call_concurrency
        .saturating_mul(ASSUMED_OUTBOUND_FANOUT)
        .max(MIN_IDLE_PER_AUTHORITY)
        // The workload's own allowance is the ceiling: keeping more idle than
        // it may ever hold open would reserve capacity it cannot use.
        .min(max_http)
        // hyper treats a cap of zero as "keep no idle connections", which
        // would defeat the pool; a budget of zero is rejected at startup, but
        // do not depend on that here.
        .max(1)
}

/// How long a workload's pooled client survives without that workload making a
/// request. Eviction drops the pool (closing its idle connections — in-flight
/// requests hold their own clone and are unaffected). Kept short and equal to
/// [`POOL_IDLE_TIMEOUT`]: workloads are typically fast-running functions, and
/// a client whose connections have all idled out anyway is just memory, so
/// there is nothing worth keeping past that window.
///
/// This TTI also backstops an isolation property: eviction plus lazy
/// re-creation gives a *fresh* TLS session-resumption store, so a workload ID
/// reused after the idle window cannot resume the previous holder's TLS
/// sessions. Raising the TTI widens the window in which a reused ID inherits
/// the prior client (workload stop already invalidates eagerly via
/// [`WorkloadClients::invalidate`], so this only matters for callers that
/// never signal unbind).
const WORKLOAD_CLIENT_IDLE: Duration = Duration::from_secs(60);

/// Shortest gap between two [`warn_permits_exhausted`] lines for one workload.
const PERMIT_WARN_INTERVAL: Duration = Duration::from_secs(5);

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

/// Whether an I/O error is a name-resolution failure.
///
/// Unix surfaces `getaddrinfo` failures with this fixed message prefix (the
/// same match wasmtime's default transport uses), or occasionally as
/// `AddrNotAvailable`. Windows surfaces them as WSA error codes — matched by
/// number, not message, because Windows error strings are localized. (The
/// Windows arm is an improvement over wasmtime's transport, which
/// misclassifies Windows resolver failures as connection-refused.)
fn is_resolver_error(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::AddrNotAvailable {
        return true;
    }
    if err
        .to_string()
        .starts_with("failed to lookup address information")
    {
        return true;
    }
    // WSAHOST_NOT_FOUND, WSATRY_AGAIN, WSANO_RECOVERY, WSANO_DATA.
    cfg!(windows) && matches!(err.raw_os_error(), Some(11001..=11004))
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
        .map_err(|e| {
            if is_resolver_error(&e) {
                ConnectError::Dns
            } else {
                ConnectError::Refused
            }
        })
}

/// Clone a TLS client configuration with a fresh, private session-resumption
/// store.
///
/// `ClientConfig::clone` shares the resumption store behind an `Arc`, and
/// rustls resumes sessions across clones — so every client (or connection)
/// built from a shared base configuration would otherwise share one TLS
/// session-ticket cache, letting an upstream server correlate two workloads
/// via a resumed session, against this module's isolation promise.
pub(crate) fn isolated_resumption(tls: &rustls::ClientConfig) -> rustls::ClientConfig {
    let mut config = tls.clone();
    config.resumption = rustls::client::Resumption::in_memory_sessions(256);
    config
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

/// Why awaiting a response head failed, before it is mapped to the P2 or P3
/// `ErrorCode` (which are distinct types).
enum HeadError {
    /// No connection became usable within the guest's `connect_timeout`.
    ConnectTimeout,
    /// A connection was in hand, but the head did not arrive within the
    /// guest's `first_byte_timeout`.
    ReadTimeout,
    /// The transport failed; [`classify_client_error`] sorts out the cause.
    Send(hyper_util::client::legacy::Error),
}

/// Resolve once `captured` has a connection, discarding the metadata.
async fn wait_connected(captured: &mut CaptureConnection) {
    let _ = captured.wait_for_connection_metadata().await;
}

/// Await a response head under wasmtime's two-phase timeout budget.
///
/// `connect_timeout` bounds only the wait for a usable connection — reusing an
/// idle pooled connection satisfies it immediately, while a fresh connect (and
/// any wait for a quota slot) must finish inside it — and
/// `first_byte_timeout` then bounds the head itself. Keeping the two phases
/// distinct preserves the guest-visible timings and error codes of wasmtime's
/// per-request transport; a single combined deadline would let a short
/// `connect_timeout` paired with a long `first_byte_timeout` wait out the sum.
///
/// hyper-util reports connection establishment through the request's
/// [`capture_connection`] extension, which it sets for reused and freshly
/// opened connections alike. That signal never arrives when connecting fails,
/// so the send future is raced alongside it to surface connect errors.
async fn send_head(
    client: &PoolClient,
    mut request: hyper::Request<ClientBody>,
    connect_timeout: Duration,
    first_byte_timeout: Duration,
) -> Result<hyper::Response<hyper::body::Incoming>, HeadError> {
    let mut captured = capture_connection(&mut request);
    let send = client.request(request);
    tokio::pin!(send);

    let settled = tokio::select! {
        biased;
        result = &mut send => Some(result),
        () = wait_connected(&mut captured) => None,
        () = tokio::time::sleep(connect_timeout) => return Err(HeadError::ConnectTimeout),
    };
    let result = match settled {
        // The send resolved before a connection was ever observed — a connect
        // failure, or a response that beat the notification.
        Some(result) => result,
        None => timeout(first_byte_timeout, send)
            .await
            .map_err(|_| HeadError::ReadTimeout)?,
    };
    result.map_err(HeadError::Send)
}

/// Which protocol a connector negotiates, over ALPN for HTTPS and by prior
/// knowledge for cleartext.
#[derive(Clone, Copy)]
enum Alpn {
    Http1,
    H2,
}

/// A pooled outbound HTTP client with configurable TLS trust roots.
///
/// Holds two pools — HTTP/1.1 for ordinary egress and HTTP/2 for the gRPC
/// fast path — drawing on one shared quota, so a workload's total
/// footprint is bounded across both protocols.
///
/// Cloning is cheap and shares the underlying connection pools.
#[derive(Clone)]
pub struct PooledClient {
    client: PoolClient,
    grpc: PoolClient,
    tls: Arc<rustls::ClientConfig>,
}

impl PooledClient {
    /// Create a standalone client using the given TLS configuration for HTTPS,
    /// bounded by a private quota of default size. Clients created through
    /// [`WorkloadClients`] instead draw on their workload's own quota and the
    /// host-wide ceiling.
    pub fn new(tls: Arc<rustls::ClientConfig>) -> Self {
        let quota = crate::host::quota::GuestConnectionQuota::new(
            crate::host::quota::QuotaLimits::default(),
            None,
        );
        Self::bounded(
            tls,
            None,
            quota.http_permits(),
            unbounded_permits(),
            crate::host::quota::QuotaRegistry::new(Default::default(), None).http_wait(),
            MIN_IDLE_PER_AUTHORITY,
        )
    }

    /// Create a client whose new connections each hold one permit from
    /// `workload_permits` (the HTTP surface of the quota belonging to the
    /// workload named by `workload`, if any) and one from `global_permits`
    /// (shared host-wide) for the connection's lifetime.
    fn bounded(
        tls: Arc<rustls::ClientConfig>,
        workload: Option<Arc<str>>,
        workload_permits: Arc<Semaphore>,
        global_permits: Arc<Semaphore>,
        permit_wait: Duration,
        idle_per_authority: usize,
    ) -> Self {
        crate::init_crypto();
        // One session store, and one exhaustion-warning throttle, shared by
        // both protocols: they belong to the workload, not to a pool.
        let tls_config = isolated_resumption(&tls);
        let last_permit_warning = Arc::new(std::sync::Mutex::new(None));
        let connector = |alpn: Alpn| {
            let mut http = HttpConnector::new();
            // The inner connector sees https URIs too; scheme handling belongs
            // to the wrapping HttpsConnector.
            http.enforce_http(false);
            http.set_nodelay(true);
            let builder = hyper_rustls::HttpsConnectorBuilder::new()
                .with_tls_config(tls_config.clone())
                .https_or_http();
            let inner = match alpn {
                Alpn::Http1 => builder.enable_http1().wrap_connector(http),
                Alpn::H2 => builder.enable_http2().wrap_connector(http),
            };
            BoundedConnector {
                inner,
                workload: workload.clone(),
                workload_permits: workload_permits.clone(),
                global_permits: global_permits.clone(),
                permit_wait,
                last_permit_warning: last_permit_warning.clone(),
            }
        };
        let pool = || {
            let mut builder = hyper_util::client::legacy::Client::builder(TokioExecutor::new());
            builder
                .pool_timer(TokioTimer::new())
                .pool_idle_timeout(POOL_IDLE_TIMEOUT)
                .pool_max_idle_per_host(idle_per_authority);
            builder
        };
        // Ordinary egress is HTTP/1.1 only, and deliberately so: an HTTP/1.1
        // pooled connection is checked out exclusively for one request at a
        // time, so a component never shares a socket even with itself. gRPC
        // has no such option — the protocol requires HTTP/2 — so its pool
        // multiplexes a workload's own concurrent streams onto one connection.
        // Both stay per-workload, which is what keeps connection-scoped server
        // state (auth, rate-limit attribution, sticky LB) from crossing
        // between components; widening either pool beyond one workload would
        // break that, whatever the protocol.
        let client = pool().build(connector(Alpn::Http1));
        let grpc = pool().http2_only(true).build(connector(Alpn::H2));
        Self { client, grpc, tls }
    }

    /// The TLS configuration this client verifies servers against.
    pub fn tls_config(&self) -> Arc<rustls::ClientConfig> {
        self.tls.clone()
    }

    /// Send a P2 outgoing request through the HTTP/1.1 pool.
    pub(crate) async fn send_request_p2(
        &self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> Result<IncomingResponse, wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> {
        self.p2(&self.client, request, config).await
    }

    /// Send a P2 gRPC request through the HTTP/2 pool.
    pub(crate) async fn send_grpc_request_p2(
        &self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> Result<IncomingResponse, wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> {
        self.p2(&self.grpc, request, config).await
    }

    async fn p2(
        &self,
        pool: &PoolClient,
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
        let resp = send_head(pool, request, connect_timeout, first_byte_timeout)
            .await
            .map_err(|err| {
                use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;
                match err {
                    HeadError::ConnectTimeout => ErrorCode::ConnectionTimeout,
                    HeadError::ReadTimeout => ErrorCode::ConnectionReadTimeout,
                    HeadError::Send(e) => classify_client_error(&e).into_p2(),
                }
            })?;
        Ok(IncomingResponse {
            resp: resp.map(|body| body.map_err(hyper_request_error).boxed_unsync()),
            // Connection lifecycle is owned by the pool; there is no
            // per-request connection task to keep alive.
            worker: None,
            between_bytes_timeout,
        })
    }

    /// Send a P3 outgoing request through the HTTP/1.1 pool.
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
        self.p3(&self.client, request, options).await
    }

    /// Send a P3 gRPC request through the HTTP/2 pool.
    pub(crate) async fn send_grpc_request_p3(
        &self,
        request: hyper::Request<P3Body>,
        options: Option<wasmtime_wasi_http::p3::RequestOptions>,
    ) -> Result<
        (hyper::Response<P3Body>, P3RequestErrorFuture),
        wasmtime_wasi_http::p3::bindings::http::types::ErrorCode,
    > {
        self.p3(&self.grpc, request, options).await
    }

    async fn p3(
        &self,
        pool: &PoolClient,
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

        let resp = send_head(pool, request, connect_timeout, first_byte_timeout)
            .await
            .map_err(|err| match err {
                HeadError::ConnectTimeout => ErrorCode::ConnectionTimeout,
                HeadError::ReadTimeout => ErrorCode::ConnectionReadTimeout,
                HeadError::Send(e) => classify_client_error(&e).into_p3(),
            })?;

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
/// host-wide ceiling.
///
/// Each workload gets its own [`PooledClient`] (created lazily on first
/// request, evicted after [`WORKLOAD_CLIENT_IDLE`] without use), so a
/// workload reuses its own keep-alive connections but components never share
/// a TCP connection with each other. Every client draws new connections from
/// its own per-workload quota and from the shared host-wide ceiling (see
/// its workload's quota).
///
/// Cloning is cheap and shares the underlying client cache.
#[derive(Clone)]
pub struct WorkloadClients {
    tls: Arc<rustls::ClientConfig>,
    /// Where each workload's allowance comes from. Held apart from
    /// [`Self::clients`] so that a client rebuilt for a workload draws on the
    /// same quota its predecessor did: a replaced client's connections keep
    /// their slots until they close, and those slots have to keep counting
    /// against the workload that opened them — a quota minted alongside each
    /// client would let one workload hold its `http` ceiling twice over while
    /// the old connections drain.
    quotas: Arc<crate::host::quota::QuotaRegistry>,
    /// Guest calls each workload may run at once, as declared by its
    /// component (see [`Self::set_call_concurrency`]), sizing its pools'
    /// idle caps. A workload absent here has not declared any, and gets
    /// [`MIN_IDLE_PER_AUTHORITY`].
    ///
    /// Kept outside the caches above because it is workload configuration
    /// rather than per-client state: it arrives when the workload binds,
    /// before any request builds a client, and is dropped when it unbinds.
    call_concurrency: Arc<std::sync::RwLock<BTreeMap<String, usize>>>,
    clients: moka::sync::Cache<String, PooledClient>,
}

impl WorkloadClients {
    /// Create a per-workload client cache using the given TLS configuration
    /// for HTTPS and a private quota registry of default size.
    pub fn new(tls: Arc<rustls::ClientConfig>) -> Self {
        Self::with_quotas(
            tls,
            crate::host::quota::QuotaRegistry::new(
                Default::default(),
                Some(crate::host::quota::DEFAULT_MAX_CONNECTIONS),
            ),
        )
    }

    /// The registry these clients draw on.
    pub fn quotas(&self) -> &Arc<crate::host::quota::QuotaRegistry> {
        &self.quotas
    }

    /// Create a per-workload client cache drawing on `quotas`.
    ///
    /// Pass the host's one registry, the same one the socket policy and the
    /// published-port publisher use, so a workload's HTTP pool and its raw
    /// sockets are bounded by one configured allowance rather than two.
    pub fn with_quotas(
        tls: Arc<rustls::ClientConfig>,
        quotas: Arc<crate::host::quota::QuotaRegistry>,
    ) -> Self {
        Self {
            tls,
            quotas,
            call_concurrency: Arc::new(std::sync::RwLock::new(BTreeMap::new())),
            clients: moka::sync::Cache::builder()
                .time_to_idle(WORKLOAD_CLIENT_IDLE)
                .build(),
        }
    }

    /// Record how many guest calls `workload_id` may run at once — call when
    /// the workload binds, before it serves anything.
    ///
    /// This sizes the idle cap of the pools built for it (see
    /// [`idle_per_authority`]): a component that keeps `pool_size` instances
    /// warm, each taking `max_concurrency` calls, bursts that many concurrent
    /// guest calls, and each of those may have several outbound requests in
    /// flight. Sizing off a fixed number instead leaves such a component
    /// churning connections once its burst outgrows it.
    ///
    /// Takes effect when the workload's client is next built; a client
    /// already serving it keeps the cap it was built with, so this is worth
    /// calling before the first request rather than after.
    pub fn set_call_concurrency(&self, workload_id: &str, calls: usize) {
        let mut declared = self
            .call_concurrency
            .write()
            .unwrap_or_else(|e| e.into_inner());
        // A workload's components each declare their own concurrency and
        // share one pool, so the busiest is what the pool has to size for.
        let entry = declared.entry(workload_id.to_string()).or_insert(calls);
        *entry = (*entry).max(calls);
    }

    /// The pooled client for `workload_id`, created on first use.
    ///
    /// `workload_id` is a trust boundary: it must be the host-assigned,
    /// unique identifier of the workload instance — never empty, never
    /// derived from guest-controllable data. Two callers presenting the same
    /// ID collapse into one pool and inherit each other's keep-alive
    /// connections and TLS session tickets (see
    /// [`crate::host::http::OutgoingHandler`]).
    pub fn client(&self, workload_id: &str) -> PooledClient {
        // Looked up on every call, not just on a client-cache miss, so the
        // quota's idle window is refreshed alongside the client's and a
        // workload's allowance cannot expire out from under a client that is
        // still serving it. The quota's window is the longer of the two, so it
        // outlives the client either way.
        let quota = self.quotas.for_guest(workload_id);
        self.clients.get_with_by_ref(workload_id, || {
            let calls = self
                .call_concurrency
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(workload_id)
                .copied()
                .unwrap_or(1);
            PooledClient::bounded(
                self.tls.clone(),
                Some(Arc::from(workload_id)),
                quota.http_permits(),
                // An unset host-wide ceiling is spelled as an effectively
                // unbounded semaphore rather than an `Option`, so the connector
                // has one acquire path instead of two.
                quota.global_permits().unwrap_or_else(unbounded_permits),
                self.quotas.http_wait(),
                idle_per_authority(calls, self.quotas.limits().http),
            )
        })
    }

    /// Drop `workload_id`'s pooled client — call when the workload stops.
    ///
    /// Closes the client's idle connections and releases their
    /// quota slots (in-flight requests hold their own clone
    /// of the client and complete unaffected; their connections close, and
    /// release their permits, when those requests finish). A subsequent
    /// [`Self::client`] call for the same ID builds a fresh client with a
    /// fresh TLS session-resumption store.
    ///
    /// The workload's quota deliberately survives, so that the
    /// draining connections stay charged to it; it ages out on its own idle
    /// window ([`WORKLOAD_CLIENT_IDLE`]) once nothing refers to the workload.
    pub fn invalidate(&self, workload_id: &str) {
        self.clients.invalidate(workload_id);
        // moka may defer dropping the evicted value to a maintenance pass;
        // force it so the pool (and the permits its idle connections pin) is
        // released now, not on the next cache access.
        self.clients.run_pending_tasks();
        self.call_concurrency
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(workload_id);
    }

    /// The TLS configuration the per-workload clients verify servers against.
    pub fn tls_config(&self) -> Arc<rustls::ClientConfig> {
        self.tls.clone()
    }
}

/// Connector that gates every *new* connection on a per-workload and a
/// host-wide semaphore (see [`crate::host::quota`]). Reusing an idle pooled
/// connection bypasses the connector entirely, so it needs no permit; hyper's
/// pool checkout races this connector against idle-connection reuse and drops
/// the pending connect (cancelling the permit acquisition) if reuse wins, so
/// waiting here never starves a request that a freed connection could serve.
#[derive(Clone)]
struct BoundedConnector {
    inner: hyper_rustls::HttpsConnector<HttpConnector>,
    /// The workload this connector belongs to, named in the exhaustion
    /// warning. `None` for a standalone [`PooledClient::new`] client.
    workload: Option<Arc<str>>,
    workload_permits: Arc<Semaphore>,
    global_permits: Arc<Semaphore>,
    permit_wait: Duration,
    /// When this workload last logged permit exhaustion; see
    /// [`warn_permits_exhausted`].
    last_permit_warning: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
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
        let workload = self.workload.clone();
        let workload_permits = self.workload_permits.clone();
        let global_permits = self.global_permits.clone();
        let permit_wait = self.permit_wait;
        let last_permit_warning = self.last_permit_warning.clone();
        Box::pin(async move {
            // Acquire order (workload, then global) is fixed everywhere, and
            // waiters hold no resource another waiter needs, so waiting on
            // both cannot deadlock. `acquire_owned` only errors when the
            // semaphore is closed, which never happens.
            let acquire =
                {
                    let workload_permits = workload_permits.clone();
                    let global_permits = global_permits.clone();
                    async move {
                        let workload = workload_permits.acquire_owned().await.map_err(|_| {
                            std::io::Error::other("outbound connection limiter closed")
                        })?;
                        let global = global_permits.acquire_owned().await.map_err(|_| {
                            std::io::Error::other("outbound connection limiter closed")
                        })?;
                        Ok::<_, std::io::Error>((workload, global))
                    }
                };
            let permits = tokio::time::timeout(permit_wait, acquire)
                .await
                .map_err(|_| {
                    warn_permits_exhausted(
                        &last_permit_warning,
                        workload.as_deref(),
                        &workload_permits,
                        &global_permits,
                    );
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

/// Log that a connect attempt gave up waiting for quota
/// permits. The guest only sees a generic connect timeout, so this is the
/// operator's signal that the connection quota — not the upstream server —
/// is what failed the request; whichever reported count is 0 is the
/// exhausted quota.
///
/// Rate-limited to one line per [`PERMIT_WARN_INTERVAL`] *per workload*: a
/// saturated quota times out every parked connect attempt in the same
/// instant (including attempts hyper abandoned after idle-connection reuse
/// won the checkout race), so an unthrottled log would flood. Throttling per
/// workload rather than host-wide keeps one workload that pins its own quota
/// from masking another workload's report of the host-wide cap.
fn warn_permits_exhausted(
    last_warning: &std::sync::Mutex<Option<std::time::Instant>>,
    workload: Option<&str>,
    workload_permits: &Semaphore,
    global_permits: &Semaphore,
) {
    let mut last = last_warning.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_none_or(|at| at.elapsed() >= PERMIT_WARN_INTERVAL) {
        *last = Some(std::time::Instant::now());
        drop(last);
        warn!(
            workload_id = workload.unwrap_or("<none>"),
            workload_permits_available = workload_permits.available_permits(),
            global_permits_available = global_permits.available_permits(),
            "outbound connect timed out waiting for a connection permit; \
             a connection quota is exhausted (logged at most once per \
             {PERMIT_WARN_INTERVAL:?} per workload)"
        );
    }
}

/// An effectively unbounded semaphore, for a host that configured no
/// host-wide ceiling.
fn unbounded_permits() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(Semaphore::MAX_PERMITS))
}

/// A connection stream carrying its quota slots; dropping
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
    /// Name resolution failed. Deliberately mapped to the same misleading
    /// `rcode="address not available"` DNS payload wasmtime's default
    /// transport emits, so guests that already match on that error shape see
    /// no change.
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

/// What the nested layers of an `io::Error` say about a failed connect.
struct IoErrorFacts {
    /// The most specific error kind found, ignoring `Other` placeholders.
    kind: std::io::ErrorKind,
    /// A rustls error is wrapped inside, so the TLS handshake is what failed.
    tls: bool,
    /// Some layer reports a name-resolution failure (see [`is_resolver_error`]).
    resolver: bool,
}

/// Drill into an `io::Error`, gathering what its nested layers say.
///
/// Every layer is inspected for all three facts, since which one carries the
/// signal varies: the resolver's message and the most specific error kind can
/// sit at different depths.
///
/// `io::Error::source()` skips the wrapped error itself (it returns the
/// *wrapped error's* source), so nested `io::Error` layers — hyper-rustls
/// wraps tokio-rustls' error, which wraps the rustls error — are only
/// reachable via `get_ref()`.
fn unwrap_io_error(io: &std::io::Error) -> IoErrorFacts {
    fn as_dyn<'a>(
        e: &'a (dyn std::error::Error + Send + Sync + 'static),
    ) -> &'a (dyn std::error::Error + Send + Sync + 'static) {
        e
    }
    let mut facts = IoErrorFacts {
        kind: io.kind(),
        tls: false,
        resolver: is_resolver_error(io),
    };
    let mut cur = io.get_ref().map(as_dyn);
    while let Some(e) = cur {
        if e.downcast_ref::<rustls::Error>().is_some() {
            facts.tls = true;
            return facts;
        }
        match e.downcast_ref::<std::io::Error>() {
            Some(inner) => {
                if inner.kind() != std::io::ErrorKind::Other {
                    facts.kind = inner.kind();
                }
                facts.resolver |= is_resolver_error(inner);
                cur = inner.get_ref().map(as_dyn);
            }
            None => break,
        }
    }
    facts
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

/// Classify a pooled-client send error into the guest-visible categories.
///
/// This walks hyper-util/hyper/rustls error chains via downcasts and matches
/// resolver failures per platform (see [`is_resolver_error`]) — a mirror of
/// wasmtime's `default_send_request` mappings that will not fail loudly if
/// those crates restructure their errors. When upgrading hyper-util, hyper,
/// or wasmtime,
/// re-diff this against `wasmtime_wasi_http`'s mapping; the
/// `connect_failures_classify_to_dns_and_refused` test pins the two
/// classifications guests most commonly match on.
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
            let facts = unwrap_io_error(io);
            if facts.tls {
                warn!(err = %format!("{err:?}"), "outbound TLS protocol error");
                return SendError::TlsProtocol;
            }
            return match facts.kind {
                std::io::ErrorKind::AddrNotAvailable => SendError::Dns,
                std::io::ErrorKind::TimedOut => SendError::ConnectionTimeout,
                _ if facts.resolver => SendError::Dns,
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

    /// Shorter than the production [`PERMIT_WAIT`]: the connection-bound tests
    /// deliberately wait out abandoned connect attempts parked on the
    /// semaphore, and the properties they assert hold at any value, so the
    /// production 5s would only slow CI and invite timing flake on loaded
    /// runners.
    const TEST_PERMIT_WAIT: Duration = Duration::from_secs(1);

    /// A quota registry with the given HTTP ceiling and host-wide cap, and the
    /// shortened [`TEST_PERMIT_WAIT`].
    fn test_quotas(http: usize, max_total: usize) -> Arc<crate::host::quota::QuotaRegistry> {
        Arc::new(
            crate::host::quota::QuotaRegistry::new(
                crate::host::quota::QuotaLimits {
                    http,
                    ..Default::default()
                },
                Some(max_total),
            )
            .as_ref()
            .clone()
            .with_http_wait(TEST_PERMIT_WAIT),
        )
    }

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
    /// The idle cap tracks declared concurrency, but never past the allowance
    /// that actually bounds the workload's connections — and an allowance
    /// below the floor must win rather than blow up the range.
    #[test]
    fn idle_cap_tracks_concurrency_within_the_quota() {
        let http = crate::host::quota::QuotaLimits::default().http;
        assert_eq!(
            idle_per_authority(1, http),
            MIN_IDLE_PER_AUTHORITY,
            "a component running one call at a time keeps the floor"
        );
        assert_eq!(
            idle_per_authority(8, http),
            8 * ASSUMED_OUTBOUND_FANOUT,
            "declared concurrency above the floor sizes the cap"
        );
        assert_eq!(
            idle_per_authority(usize::MAX, http),
            http,
            "the workload's quota is the ceiling, and the fan-out must not overflow"
        );
        assert_eq!(
            idle_per_authority(1, 4),
            4,
            "an allowance under the floor is the cap, not a panic"
        );
        assert_eq!(
            idle_per_authority(1, 1),
            1,
            "the smallest usable allowance still leaves room for one idle connection"
        );
    }

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

    /// Cleartext HTTP/2 (h2c, prior knowledge) server that counts accepted
    /// connections and answers every request with `200 ok` — the shape a gRPC
    /// backend presents.
    async fn spawn_counting_h2c_server()
    -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let conns_clone = conns.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(_) => return,
                };
                conns_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(
                        |_req: hyper::Request<hyper::body::Incoming>| async {
                            Ok::<_, std::convert::Infallible>(hyper::Response::new(
                                http_body_util::Full::new(Bytes::from_static(b"ok")),
                            ))
                        },
                    );
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        (addr, conns)
    }

    fn grpc_request(uri: &str) -> hyper::Request<HyperOutgoingBody> {
        hyper::Request::builder()
            .uri(uri)
            .method(hyper::Method::POST)
            .header(hyper::header::CONTENT_TYPE, "application/grpc")
            .body(HyperOutgoingBody::default())
            .unwrap()
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

    fn p3_request(uri: &str) -> hyper::Request<P3Body> {
        hyper::Request::builder()
            .uri(uri)
            .body(P3Body::new(
                http_body_util::Empty::new().map_err(|_: std::convert::Infallible| unreachable!()),
            ))
            .unwrap()
    }

    /// A guest may set any `between-bytes-timeout`, including zero, and
    /// `tokio::time::interval` panics on a zero period — so the response-body
    /// wrapper must clamp it rather than take the host down.
    #[tokio::test]
    async fn zero_between_bytes_timeout_is_not_a_panic() {
        let (addr, _conns) = spawn_counting_server().await;
        let client = PooledClient::new(default_client_tls_config());
        let options = wasmtime_wasi_http::p3::RequestOptions {
            connect_timeout: None,
            first_byte_timeout: None,
            between_bytes_timeout: Some(Duration::ZERO),
        };

        let (response, _io) = client
            .send_request_p3(p3_request(&format!("http://{addr}/")), Some(options))
            .await
            .expect("a zero between-bytes timeout must not fail the request head");
        assert_eq!(response.status(), 200);
        // Whether the body reads or times out depends on frame arrival; the
        // point is that it resolves instead of panicking.
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            BodyExt::collect(response.into_body()),
        )
        .await
        .expect("body read should resolve");
    }

    /// `connect_timeout` must bound only the wait for a connection, not the
    /// response head: a server that is quick to accept but slow to answer must
    /// still get the full `first_byte_timeout`.
    #[tokio::test]
    async fn connect_timeout_does_not_bound_the_response_head() {
        let (addr, _conns) = spawn_counting_server_with_delay(Duration::from_millis(400)).await;
        let client = PooledClient::new(default_client_tls_config());

        let response = client
            .send_request_p2(
                p2_request(&format!("http://{addr}/")),
                OutgoingRequestConfig {
                    use_tls: false,
                    connect_timeout: Duration::from_millis(150),
                    first_byte_timeout: Duration::from_secs(5),
                    between_bytes_timeout: Duration::from_secs(5),
                },
            )
            .await
            .expect("a slow head must not be charged against the connect deadline");
        assert_eq!(response.resp.status(), 200);
    }

    /// When no connection can be had, it is the guest's `connect_timeout`
    /// that must end the request — with `ConnectionTimeout`, and at its own
    /// deadline rather than at whatever the transport happens to give up on.
    /// Starvation is forced with a host-wide cap of one connection held by a
    /// slow in-flight request.
    ///
    /// The elapsed-time bound is what makes this test meaningful: a request
    /// blocked on a permit also ends at the quota's http wait, so
    /// only a failure that lands well inside that window shows the guest's
    /// deadline was the one being honoured.
    #[tokio::test]
    async fn connect_timeout_fires_before_the_first_byte_deadline() {
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

        let connect_timeout = TEST_PERMIT_WAIT / 5;
        let (addr, _conns) = spawn_counting_server_with_delay(TEST_PERMIT_WAIT * 2).await;
        let clients = WorkloadClients::with_quotas(default_client_tls_config(), test_quotas(1, 1));
        let uri = format!("http://{addr}/");

        // Occupy the only permit for the duration of the slow request.
        let busy = clients.client("workload-a");
        let busy_uri = uri.clone();
        let busy = tokio::spawn(async move {
            let response = busy
                .send_request_p2(p2_request(&busy_uri), p2_config(false))
                .await
                .expect("the first request should get the only connection");
            let _ = response.resp.into_body().collect().await;
        });
        tokio::time::sleep(Duration::from_millis(150)).await;

        let started = tokio::time::Instant::now();
        let err = clients
            .client("workload-b")
            .send_request_p2(
                p2_request(&uri),
                OutgoingRequestConfig {
                    use_tls: false,
                    connect_timeout,
                    first_byte_timeout: Duration::from_secs(30),
                    between_bytes_timeout: Duration::from_secs(30),
                },
            )
            .await
            .expect_err("no slot is available, so the connect deadline must expire");
        let elapsed = started.elapsed();

        assert!(
            matches!(err, ErrorCode::ConnectionTimeout),
            "expected ConnectionTimeout, got {err:?}"
        );
        assert!(
            elapsed < TEST_PERMIT_WAIT,
            "the guest's {connect_timeout:?} connect deadline must end the request, \
             but it took {elapsed:?} — at or past the {TEST_PERMIT_WAIT:?} permit deadline, \
             so the guest's deadline was not what bounded it"
        );
        let _ = busy.await;
    }

    /// Stopping a workload must drop its pooled client: the next client for
    /// the same ID gets a fresh pool (and a fresh TLS session-resumption
    /// store) instead of inheriting the previous holder's connections.
    #[tokio::test]
    async fn invalidate_drops_pooled_connections() {
        let (addr, conns) = spawn_counting_server().await;
        let clients = WorkloadClients::new(default_client_tls_config());
        let uri = format!("http://{addr}/");

        let client = clients.client("workload-a");
        let response = client
            .send_request_p2(p2_request(&uri), p2_config(false))
            .await
            .expect("request should succeed");
        let _ = response.resp.into_body().collect().await;
        drop(client);
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 1);

        clients.invalidate("workload-a");

        let client = clients.client("workload-a");
        let response = client
            .send_request_p2(p2_request(&uri), p2_config(false))
            .await
            .expect("request should succeed");
        let _ = response.resp.into_body().collect().await;
        assert_eq!(
            conns.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "invalidate must drop the old pool so the rebuilt client cannot reuse its connections"
        );
    }

    /// gRPC egress must reuse pooled HTTP/2 connections rather than opening
    /// one per request — the same ephemeral-port exhaustion this module
    /// exists to prevent applies to a component talking gRPC.
    #[tokio::test]
    async fn grpc_requests_reuse_the_pooled_h2_connection() {
        let (addr, conns) = spawn_counting_h2c_server().await;
        let client = PooledClient::new(default_client_tls_config());
        let uri = format!("http://{addr}/svc.Test/Call");

        for _ in 0..20 {
            let response = client
                .send_grpc_request_p2(grpc_request(&uri), p2_config(false))
                .await
                .expect("gRPC request should succeed");
            assert_eq!(response.resp.status(), 200);
            let _ = response.resp.into_body().collect().await;
        }

        let opened = conns.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            opened <= 2,
            "expected connection reuse across 20 sequential gRPC requests, \
             but the server saw {opened} connections"
        );
    }

    /// gRPC and ordinary egress draw on one quota surface, so a workload
    /// cannot evade its cap by switching protocols.
    #[tokio::test]
    async fn grpc_and_http_egress_share_the_workload_quota() {
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

        let (grpc_addr, _grpc_conns) = spawn_counting_h2c_server().await;
        let (http_addr, http_conns) = spawn_counting_server().await;
        let clients =
            WorkloadClients::with_quotas(default_client_tls_config(), test_quotas(1, 100));
        let client = clients.client("workload-a");

        // One gRPC connection, left idle in the h2 pool still holding the
        // workload's only permit.
        let response = client
            .send_grpc_request_p2(
                grpc_request(&format!("http://{grpc_addr}/svc.Test/Call")),
                p2_config(false),
            )
            .await
            .expect("gRPC request should succeed");
        let _ = response.resp.into_body().collect().await;

        let err = client
            .send_request_p2(
                p2_request(&format!("http://{http_addr}/")),
                OutgoingRequestConfig {
                    use_tls: false,
                    connect_timeout: TEST_PERMIT_WAIT / 5,
                    first_byte_timeout: Duration::from_secs(30),
                    between_bytes_timeout: Duration::from_secs(30),
                },
            )
            .await
            .expect_err("the gRPC connection holds the workload's only permit");
        assert!(
            matches!(err, ErrorCode::ConnectionTimeout),
            "expected ConnectionTimeout, got {err:?}"
        );
        assert_eq!(
            http_conns.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no HTTP/1.1 connection should have been opened over the ceiling"
        );
    }

    /// Replacing a workload's client must not hand it a second connection
    /// quota: the outgoing client's connections keep their slots until they
    /// close, so a fresh quota would let one workload hold
    /// `max_per_workload` twice over while the old connections drain.
    #[tokio::test]
    async fn rebuilt_client_shares_the_workload_quota() {
        let (addr, _conns) = spawn_counting_server_with_delay(TEST_PERMIT_WAIT * 2).await;
        let clients =
            WorkloadClients::with_quotas(default_client_tls_config(), test_quotas(1, 100));
        let uri = format!("http://{addr}/");

        // A request slow enough to still hold the workload's only connection
        // when it is invalidated below.
        let busy_client = clients.client("workload-a");
        let busy_uri = uri.clone();
        let busy = tokio::spawn(async move {
            if let Ok(response) = busy_client
                .send_request_p2(p2_request(&busy_uri), p2_config(false))
                .await
            {
                let _ = response.resp.into_body().collect().await;
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;

        let quota = clients.quotas().for_guest("workload-a");
        assert_eq!(
            quota.http_available(),
            0,
            "the in-flight connection should hold the workload's only slot"
        );

        clients.invalidate("workload-a");
        let _rebuilt = clients.client("workload-a");
        let rebuilt_quota = clients.quotas().for_guest("workload-a");
        assert_eq!(
            rebuilt_quota.http_available(),
            0,
            "the draining connection must still be charged to the workload"
        );

        busy.abort();
        let _ = busy.await;
    }

    /// Resolver failures and refused connects must keep classifying to the
    /// error codes wasmtime's default transport produces — guests match on
    /// these, and `classify_client_error`'s chain-walking would rot silently
    /// on a hyper-util upgrade otherwise. Runs on every platform: Windows
    /// resolver failures are matched by WSA error code (see
    /// [`is_resolver_error`]), where wasmtime's own transport misclassifies
    /// them as connection-refused.
    #[tokio::test]
    async fn connect_failures_classify_to_dns_and_refused() {
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

        let client = PooledClient::new(default_client_tls_config());

        // RFC 6761 reserves `.invalid`: resolution always fails.
        let err = client
            .send_request_p2(
                p2_request("http://definitely-not-a-real-host.invalid/"),
                p2_config(false),
            )
            .await
            .expect_err("resolution must fail");
        assert!(
            matches!(err, ErrorCode::DnsError(_)),
            "expected DnsError, got {err:?}"
        );

        // A loopback port nothing listens on: TCP connect is refused.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let err = client
            .send_request_p2(
                p2_request(&format!("http://127.0.0.1:{port}/")),
                p2_config(false),
            )
            .await
            .expect_err("connect must be refused");
        assert!(
            matches!(err, ErrorCode::ConnectionRefused),
            "expected ConnectionRefused, got {err:?}"
        );
    }

    /// Serve one HTTP/1.1 request on an ephemeral port, sending the head and
    /// then the body `body_delay` later. Returns the port.
    async fn delayed_body_server(body_delay: Duration) -> u16 {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let mut seen = Vec::new();
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => seen.extend_from_slice(&buf[..n]),
                }
            }
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\n")
                .await;
            let _ = sock.flush().await;
            tokio::time::sleep(body_delay).await;
            let _ = sock.write_all(b"hello").await;
            let _ = sock.flush().await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        port
    }

    /// The response body must keep streaming after the head on the pooled P3
    /// path, including when the request-error future has already been driven
    /// to completion — connection lifetime belongs to the pool, not to that
    /// future.
    #[tokio::test]
    async fn p3_body_streams_after_head_on_pooled_connection() {
        let port = delayed_body_server(Duration::from_millis(300)).await;
        let client = PooledClient::new(default_client_tls_config());
        let (response, io) = client
            .send_request_p3(p3_request(&format!("http://127.0.0.1:{port}/")), None)
            .await
            .expect("request should succeed");

        // Drive the request-error future the way wasmtime does.
        let io = wasmtime_wasi::runtime::spawn(async move { Box::into_pin(io).await });
        let body = tokio::time::timeout(
            Duration::from_secs(3),
            BodyExt::collect(response.into_body()),
        )
        .await
        .expect("body read timed out")
        .expect("body read failed");
        assert_eq!(body.to_bytes().as_ref(), b"hello");
        drop(io);
    }

    /// A server that hangs up mid-body must surface as an error on the
    /// response body, not as a silently truncated success. On the pooled path
    /// this guarantee is enforced by hyper's content-length check surfacing
    /// through `TimedBody`'s error mapping — pin it here (ported from the
    /// per-request transport's test suite).
    #[tokio::test]
    async fn p3_truncated_body_surfaces_as_body_error() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            // Promise five bytes, then hang up without sending them.
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\n")
                .await;
            let _ = sock.flush().await;
            drop(sock);
        });

        let client = PooledClient::new(default_client_tls_config());
        let (response, io) = client
            .send_request_p3(p3_request(&format!("http://127.0.0.1:{port}/")), None)
            .await
            .expect("response head should arrive");
        let io = wasmtime_wasi::runtime::spawn(async move { Box::into_pin(io).await });

        let err = tokio::time::timeout(
            Duration::from_secs(3),
            BodyExt::collect(response.into_body()),
        )
        .await
        .expect("body read should not hang")
        .expect_err("a truncated body must not read as success");
        assert!(
            matches!(
                err,
                wasmtime_wasi_http::p3::bindings::http::types::ErrorCode::HttpProtocolError
            ),
            "expected HttpProtocolError, got {err:?}"
        );
        drop(io);
    }

    /// The per-workload connection bound must hold under a concurrent burst:
    /// with a cap of 2, eight concurrent requests to a slow server must be
    /// funnelled through at most two connections (waiters pick up pooled
    /// connections as they free instead of opening new ones).
    #[tokio::test]
    async fn per_workload_connection_bound_holds_under_burst() {
        let (addr, conns) = spawn_counting_server_with_delay(Duration::from_millis(50)).await;
        let clients =
            WorkloadClients::with_quotas(default_client_tls_config(), test_quotas(2, 100));
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
        let clients = WorkloadClients::with_quotas(default_client_tls_config(), test_quotas(4, 2));
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
        // give up at [`TEST_PERMIT_WAIT`]; the pool then drops, closing A's
        // idle connections and releasing their permits.
        clients.invalidate("workload-a");
        let deadline = tokio::time::Instant::now() + TEST_PERMIT_WAIT + Duration::from_secs(3);
        let host_wide = clients
            .quotas()
            .for_guest("probe")
            .global_permits()
            .expect("the test registry configures a host-wide ceiling");
        while host_wide.available_permits() == 0 {
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
