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
//! trusts only the compiled-in webpki roots, with no way to reach
//! hosts behind a corporate or private CA. [`ClientTlsOptions`] builds a root
//! store from a [`TrustRoots`] base (webpki and/or the platform's native
//! store, which honours `SSL_CERT_FILE`/`SSL_CERT_DIR`) with any explicitly
//! configured PEM bundles layered on top.
//!
//! The per-connection helpers ([`connect_http_tcp`], [`connect_http_tls`], the
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
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use tokio::net::TcpStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;
use tracing::{debug, warn};
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi_http::{Error as HttpError, RequestOptions, WasiBody};

use crate::host::http::{RequestIoFuture, SendResult};

/// Error type carried by the request body handed to the pooled client.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Request body the pooled client sends.
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
    /// Compiled-in webpki roots plus the platform's native store.
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

/// PEM files holding a client certificate chain and its private key.
///
/// The other half of the egress trust store from
/// [`ClientTlsOptions::extra_ca_paths`]: the bundles there decide which
/// servers the host will talk to, this decides who it says it is when one
/// asks. A peer that only *requests* a certificate completes the handshake
/// either way, so a missing identity surfaces as the upstream rejecting
/// requests rather than as a TLS error.
///
/// `#[non_exhaustive]`: PEM files on disk are the only form today, but a
/// credential can arrive as a PKCS#12 bundle, as DER already in memory, or as
/// a handle to something that never leaves an HSM. Naming the shape now keeps
/// adding one from being a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientIdentity {
    /// A certificate chain and its private key, each PEM-encoded on disk.
    CertificatePem {
        /// Certificate chain, leaf first.
        cert_path: PathBuf,
        /// Private key for the leaf certificate.
        key_path: PathBuf,
    },
}

impl std::fmt::Display for ClientIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CertificatePem {
                cert_path,
                key_path,
            } => write!(
                f,
                "certificate {} with key {}",
                cert_path.display(),
                key_path.display()
            ),
        }
    }
}

impl ClientIdentity {
    /// Read the chain and key.
    pub(crate) fn load(
        &self,
    ) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let Self::CertificatePem {
            cert_path,
            key_path,
        } = self;
        let certs = CertificateDer::pem_file_iter(cert_path)
            .with_context(|| format!("failed to read client certificate {}", cert_path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| {
                format!(
                    "failed to parse PEM in client certificate {}",
                    cert_path.display()
                )
            })?;
        anyhow::ensure!(
            !certs.is_empty(),
            "no certificate found in {}",
            cert_path.display()
        );

        let key = PrivateKeyDer::from_pem_file(key_path)
            .with_context(|| format!("failed to read client private key {}", key_path.display()))?;
        Ok((certs, key))
    }
}

/// Trust-root and client-identity options for outbound HTTPS from components.
///
/// `#[non_exhaustive]`: build one with [`Default`] and the `with_*` methods
/// rather than a struct literal, so that adding an option later is not a
/// breaking change for callers outside this crate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientTlsOptions {
    /// Built-in roots to start from.
    pub roots: TrustRoots,
    /// Additional PEM CA bundle files to trust (each file may contain one or
    /// more certificates), layered on top of `roots`. Use this to reach hosts
    /// behind a corporate or otherwise private CA.
    pub extra_ca_paths: Vec<PathBuf>,
    /// Client certificate the host presents when a peer requests one. `None`
    /// presents nothing, which is what an unconfigured host does.
    ///
    /// Loading this once at build time is deliberate: a credential that only
    /// fails at handshake time surfaces as a peer rejecting every request,
    /// which reads like a broken upstream.
    ///
    /// Validity is *not* tracked. The pair is checked for consistency when it
    /// is loaded and then presented for the life of the configuration, so a
    /// long-running host will keep offering it past `notAfter`. A host that
    /// needs either rotation or expiry handling should install its own
    /// [`rustls::client::ResolvesClientCert`] on the built configuration,
    /// which rustls consults once per handshake and which can decline.
    pub client_identity: Option<ClientIdentity>,
}

impl ClientTlsOptions {
    /// Options trusting `roots` and nothing else yet.
    #[must_use]
    pub fn new(roots: TrustRoots) -> Self {
        Self {
            roots,
            ..Default::default()
        }
    }

    /// Also trust the CA bundles at `paths`.
    #[must_use]
    pub fn with_ca_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.extra_ca_paths.extend(paths);
        self
    }

    /// Present `identity` when a peer requests a client certificate.
    #[must_use]
    pub fn with_client_identity(mut self, identity: ClientIdentity) -> Self {
        self.client_identity = Some(identity);
        self
    }

    /// Build a rustls client configuration from these options.
    ///
    /// Fails when an entry in `extra_ca_paths` cannot be read or contains no
    /// usable certificate, when the options yield an empty trust store
    /// (e.g. [`TrustRoots::ExtraOnly`] with no bundles), or when
    /// `client_identity` names an unreadable or mismatched pair; problems
    /// loading individual native-store certificates are logged and skipped.
    pub fn build(&self) -> anyhow::Result<Arc<rustls::ClientConfig>> {
        // Resolved first: `root_store` installs the crypto provider that
        // `ClientConfig::builder` panics without, and as the receiver the
        // builder would otherwise be evaluated before it.
        let roots = self.root_store()?;
        let builder = rustls::ClientConfig::builder().with_root_certificates(roots);
        let config = match &self.client_identity {
            Some(identity) => {
                let (certs, key) = identity.load()?;
                // Parses the key and compares it against the leaf's
                // SubjectPublicKeyInfo, so this rejects both a key the
                // provider cannot use and a crossed pair. Not a guarantee of
                // consistency: `CertifiedKey::from_der` tolerates
                // `InconsistentKeys(Unknown)`, so a key that cannot report its
                // public half is accepted unchecked.
                let config = builder
                    .with_client_auth_cert(certs, key)
                    .with_context(|| format!("{identity} is not a usable client identity"))?;
                debug!(
                    identity = %identity,
                    "presenting a client certificate for outbound TLS"
                );
                config
            }
            None => builder.with_no_client_auth(),
        };

        Ok(Arc::new(config))
    }

    /// Build a rustls client configuration whose client identity comes from
    /// `resolver` rather than from [`Self::client_identity`].
    ///
    /// Uses the same trust roots as [`Self::build`] and ignores
    /// [`Self::client_identity`]. Session resumption is disabled so each new
    /// connection consults the resolver.
    pub fn build_with_resolver(
        &self,
        resolver: Arc<dyn rustls::client::ResolvesClientCert>,
    ) -> anyhow::Result<Arc<rustls::ClientConfig>> {
        let roots = self.root_store()?;
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_cert_resolver(resolver);
        config.resumption = rustls::client::Resumption::disabled();
        Ok(Arc::new(config))
    }

    /// The trust store these options describe, without deciding how the client
    /// authenticates itself.
    ///
    /// Split out for the OTLP exporters, which trust a collector the same way
    /// but may also present a client certificate to it.
    pub(crate) fn root_store(&self) -> anyhow::Result<rustls::RootCertStore> {
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

        Ok(roots)
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

/// A name-resolution failure as wasmtime's `default_send_request` reports it.
///
/// `rcode="address not available"` is what it emits for every resolver
/// failure — misleading, but the error shape guests already match on, so both
/// transports here emit it too.
fn dns_error(rcode: &str) -> HttpError {
    HttpError::DnsError {
        rcode: Some(rcode.to_string()),
        info_code: Some(0),
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

/// Open an HTTP TCP connection and return guest-visible connection errors.
pub(crate) async fn connect_http_tcp(
    authority: &str,
    connect_timeout: Duration,
) -> Result<TcpStream, HttpError> {
    timeout(connect_timeout, TcpStream::connect(authority))
        .await
        .map_err(|_| HttpError::ConnectionTimeout)?
        .map_err(|e| {
            if is_resolver_error(&e) {
                dns_error("address not available")
            } else {
                HttpError::ConnectionRefused
            }
        })
}

/// Clone a TLS client configuration with a fresh, private session-resumption
/// store — or none, for one that presents a client certificate.
///
/// `ClientConfig::clone` shares the resumption store behind an `Arc`, and
/// rustls resumes sessions across clones — so every client (or connection)
/// built from a shared base configuration would otherwise share one TLS
/// session-ticket cache, letting an upstream server correlate two workloads
/// via a resumed session, against this module's isolation promise.
///
/// A resumed session skips the client-certificate resolver and keeps the
/// authentication it was established with, so a configuration presenting an
/// identity does not resume at all: a rotated or expired credential must stop
/// authenticating new connections, which a session cached under the old one
/// would keep doing.
pub(crate) fn isolated_resumption(tls: &rustls::ClientConfig) -> rustls::ClientConfig {
    let mut config = tls.clone();
    config.resumption = if tls.client_auth_cert_resolver.has_certs() {
        rustls::client::Resumption::disabled()
    } else {
        rustls::client::Resumption::in_memory_sessions(256)
    };
    config
}

/// Run a TLS client handshake over an established TCP stream, using
/// `authority`'s host portion as the SNI server name.
pub(crate) async fn connect_http_tls(
    tls: Arc<rustls::ClientConfig>,
    authority: &str,
    tcp_stream: TcpStream,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, HttpError> {
    let connector = tokio_rustls::TlsConnector::from(tls);
    let domain = tls_server_name(authority).ok_or_else(|| {
        warn!(authority = %authority, "invalid TLS server name");
        dns_error("invalid dns name")
    })?;
    connector.connect(domain, tcp_stream).await.map_err(|e| {
        warn!("tls protocol error: {e:?}");
        HttpError::TlsProtocolError
    })
}

/// Spawn the hyper connection driver for an egress connection.
///
/// The returned handle doubles as the request's [`RequestIoFuture`], so a
/// connection failure is propagated to the guest via [`connection_error`]
/// rather than dropped.
pub(crate) fn spawn_conn_worker<F>(conn: F) -> AbortOnDropJoinHandle<Result<(), HttpError>>
where
    F: Future<Output = Result<(), hyper::Error>> + Send + 'static,
{
    wasmtime_wasi::runtime::spawn(async move { conn.await.map_err(connection_error) })
}

/// Translate an error from a hyper connection or response body, as wasmtime's
/// `default_send_request` does: a timeout becomes `HttpResponseTimeout`, and
/// anything else is left for wasmtime to classify.
pub(crate) fn connection_error(err: hyper::Error) -> HttpError {
    if err.is_timeout() {
        HttpError::HttpResponseTimeout
    } else {
        HttpError::Hyper(err)
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

/// Why awaiting a response head failed.
///
/// A timeout is already the guest's error: `connect_timeout` bounds the wait
/// for a usable connection, `first_byte_timeout` the head itself. A transport
/// failure is not yet — it may be hyper's wrapper around the guest's own body
/// error, which only the caller of [`send_head`] can see.
enum HeadError {
    Timeout(HttpError),
    Transport(hyper_util::client::legacy::Error),
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
        () = tokio::time::sleep(connect_timeout) => {
            return Err(HeadError::Timeout(HttpError::ConnectionTimeout));
        }
    };
    let result = match settled {
        // The send resolved before a connection was ever observed — a connect
        // failure, or a response that beat the notification.
        Some(result) => result,
        None => timeout(first_byte_timeout, send)
            .await
            .map_err(|_| HeadError::Timeout(HttpError::ConnectionReadTimeout))?,
    };
    result.map_err(HeadError::Transport)
}

/// Which protocol a connector negotiates, over ALPN for HTTPS and by prior
/// knowledge for cleartext.
#[derive(Clone, Copy)]
pub(crate) enum Alpn {
    Http1,
    H2,
}

/// The HTTPS connector every outbound connection this host makes is built on.
/// A plain `HttpConnector` with `nodelay`, wrapped so it also speaks TLS.
pub(crate) fn https_connector(
    tls: &rustls::ClientConfig,
    alpn: Alpn,
) -> hyper_rustls::HttpsConnector<HttpConnector> {
    crate::init_crypto();
    let mut http = HttpConnector::new();
    // The inner connector sees https URIs too; scheme handling belongs
    // to the wrapping HttpsConnector.
    http.enforce_http(false);
    http.set_nodelay(true);
    let builder = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls.clone())
        .https_or_http();
    match alpn {
        Alpn::Http1 => builder.enable_http1().wrap_connector(http),
        Alpn::H2 => builder.enable_http2().wrap_connector(http),
    }
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
            quota.outbound_http_permits(),
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
        let connector = |alpn: Alpn| BoundedConnector {
            inner: https_connector(&tls_config, alpn),
            workload: workload.clone(),
            workload_permits: workload_permits.clone(),
            global_permits: global_permits.clone(),
            permit_wait,
            last_permit_warning: last_permit_warning.clone(),
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

    /// Send an outgoing request through the HTTP/1.1 pool.
    ///
    /// The returned future reports the request-body upload outcome to the
    /// guest: `Ok(())` once the body has been fully pulled, or the body's own
    /// error if producing it failed.
    pub(crate) async fn send_request(
        &self,
        request: hyper::Request<WasiBody>,
        options: Option<RequestOptions>,
    ) -> SendResult {
        self.send(&self.client, request, options).await
    }

    /// Send a gRPC request through the HTTP/2 pool.
    pub(crate) async fn send_grpc_request(
        &self,
        request: hyper::Request<WasiBody>,
        options: Option<RequestOptions>,
    ) -> SendResult {
        self.send(&self.grpc, request, options).await
    }

    async fn send(
        &self,
        pool: &PoolClient,
        request: hyper::Request<WasiBody>,
        options: Option<RequestOptions>,
    ) -> SendResult {
        let connect_timeout = options
            .and_then(|o| o.connect_timeout)
            .unwrap_or(Duration::from_secs(600));
        let first_byte_timeout = options
            .and_then(|o| o.first_byte_timeout)
            .unwrap_or(Duration::from_secs(600));
        let between_bytes_timeout = options
            .and_then(|o| o.between_bytes_timeout)
            .unwrap_or(Duration::from_secs(600));

        let (parts, body) = request.into_parts();
        let (body, mut upload_rx) = UploadProbe::new(body);
        let body = body.map_err(|e| Box::new(e) as BoxError).boxed_unsync();
        let request = hyper::Request::from_parts(parts, body);

        let resp = send_head(pool, request, connect_timeout, first_byte_timeout)
            .await
            .map_err(|err| match err {
                HeadError::Timeout(err) => err,
                // The guest's own body error, when that is what failed, rather
                // than the transport error hyper wrapped the marker in.
                HeadError::Transport(e) => match upload_rx.try_recv() {
                    Ok(Err(body_err)) => body_err,
                    _ => classify_client_error(&e),
                },
            })?;

        let resp = resp.map(|body| {
            crate::host::http::TimedBody::new(body, between_bytes_timeout).boxed_unsync()
        });

        Ok((resp, upload_io(upload_rx)))
    }
}

/// Supplies the TLS configuration a workload's outbound connections use.
///
/// A host that gives each workload its own client identity implements this;
/// one that does not passes an `Arc<rustls::ClientConfig>`, which implements
/// it by handing the same configuration to everyone.
///
/// Consulted when a workload's [`PooledClient`] is built, not per request, so
/// a configuration handed out here is the one every connection in that pool
/// negotiates with.
///
/// Must be cheap and must not block. It is called synchronously on the
/// runtime worker serving the workload's first outbound request, while the
/// client cache holds its per-key initialization lock, so reading a file or
/// fetching a secret here stalls that worker and serializes every concurrent
/// first request for the workload. Resolve credentials ahead of time and let
/// this hand back what is already loaded. Replace a credential *without* rebuilding the pool by
/// keeping one configuration per workload whose
/// [`rustls::client::ResolvesClientCert`] reads swappable state: rustls
/// consults that on every handshake, so a rotated credential applies to new
/// connections while established ones keep what they negotiated with.
///
/// `workload_id` is the host-assigned identifier described on
/// [`WorkloadClients::client`], never guest-controlled, which is what makes
/// it safe to key an identity on.
pub trait ClientTlsConfigResolver: Send + Sync + 'static {
    /// The configuration `workload_id`'s connections use.
    fn config_for(&self, workload_id: &str) -> Arc<rustls::ClientConfig>;

    /// The configuration for egress that belongs to no single workload.
    ///
    /// Reached through [`OutgoingHandler::client_tls_config`] on the gRPC
    /// fallback path, which only handlers that do not pool per workload take:
    /// [`DefaultOutgoingHandler`] answers gRPC from the workload's own pooled
    /// client, so it keeps that workload's identity.
    ///
    /// [`OutgoingHandler::client_tls_config`]: crate::host::http::OutgoingHandler::client_tls_config
    /// [`DefaultOutgoingHandler`]: crate::host::http::DefaultOutgoingHandler
    fn host_config(&self) -> Arc<rustls::ClientConfig>;
}

/// One configuration for every workload, which is what a host without
/// per-workload identity wants.
impl ClientTlsConfigResolver for Arc<rustls::ClientConfig> {
    fn config_for(&self, _workload_id: &str) -> Arc<rustls::ClientConfig> {
        Arc::clone(self)
    }

    fn host_config(&self) -> Arc<rustls::ClientConfig> {
        Arc::clone(self)
    }
}

/// Per-workload pooled clients sharing one host-wide ceiling.
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
    tls: Arc<dyn ClientTlsConfigResolver>,
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
                Some(crate::host::quota::default_max_connections()),
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
        Self::with_tls_config_resolver(Arc::new(tls), quotas)
    }

    /// Create a per-workload client cache that resolves a TLS configuration
    /// per workload, so each can present its own client identity.
    ///
    /// Otherwise identical to [`Self::with_quotas`], which is this with a
    /// resolver that answers every workload the same way.
    pub fn with_tls_config_resolver(
        tls: Arc<dyn ClientTlsConfigResolver>,
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
                self.tls.config_for(workload_id),
                Some(Arc::from(workload_id)),
                quota.outbound_http_permits(),
                // An unset host-wide ceiling is spelled as an effectively
                // unbounded semaphore rather than an `Option`, so the connector
                // has one acquire path instead of two.
                quota.global_permits().unwrap_or_else(unbounded_permits),
                self.quotas.http_wait(),
                idle_per_authority(calls, self.quotas.limits().outbound_http),
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

    /// The TLS configuration for egress belonging to no single workload.
    ///
    /// Not necessarily what any given workload's connections use: a host with
    /// per-workload identities resolves those in [`Self::client`]. See
    /// [`ClientTlsConfigResolver::host_config`].
    pub fn tls_config(&self) -> Arc<rustls::ClientConfig> {
        self.tls.host_config()
    }

    /// The resolver these clients draw their configurations from.
    ///
    /// Rebuilding a cache (as [`DefaultOutgoingHandler::with_quotas`] does)
    /// has to carry this over: taking [`Self::tls_config`] instead would
    /// collapse every workload onto the host-wide configuration.
    ///
    /// [`DefaultOutgoingHandler::with_quotas`]: crate::host::http::DefaultOutgoingHandler::with_quotas
    pub fn tls_config_resolver(&self) -> Arc<dyn ClientTlsConfigResolver> {
        Arc::clone(&self.tls)
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
///
/// A failure is *moved* into the oneshot, and hyper is handed a marker in its
/// place: [`HttpError`] is `#[non_exhaustive]` and not `Clone`, so copying one
/// would mean maintaining a hand-written clone of an upstream enum. The
/// oneshot is the only reader that matters — [`PooledClient::send`] takes the
/// error from it, whether the request failed before the head or after it.
pub(crate) struct UploadProbe {
    inner: WasiBody,
    done: Option<tokio::sync::oneshot::Sender<Result<(), HttpError>>>,
}

/// The outcome channel of an [`UploadProbe`], which a sender both reads
/// directly — to prefer the guest's own body error over hyper's wrapper — and
/// hands to the guest as its request-error future.
pub(crate) type UploadOutcome = tokio::sync::oneshot::Receiver<Result<(), HttpError>>;

impl UploadProbe {
    /// Wrap `inner`, returning the channel its upload outcome arrives on.
    pub(crate) fn new(inner: WasiBody) -> (Self, UploadOutcome) {
        let (done, outcome) = tokio::sync::oneshot::channel();
        (
            Self {
                inner,
                done: Some(done),
            },
            outcome,
        )
    }
}

/// The guest's request-error future for a body wrapped by [`UploadProbe`]. A
/// body dropped before completing, e.g. by a server that responded without
/// draining it, is not a guest-visible failure.
pub(crate) fn upload_io(outcome: UploadOutcome) -> RequestIoFuture {
    Box::new(async move { outcome.await.unwrap_or(Ok(())) })
}

impl hyper::body::Body for UploadProbe {
    type Data = Bytes;
    type Error = HttpError;

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
                match self.done.take() {
                    // Sent before hyper can observe the failure, so `send` finds
                    // it waiting.
                    Some(done) => {
                        let _ = done.send(Err(err));
                        Poll::Ready(Some(Err(HttpError::InternalError(Some(
                            "request body failed".to_string(),
                        )))))
                    }
                    None => Poll::Ready(Some(Err(err))),
                }
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
fn classify_client_error(err: &hyper_util::client::legacy::Error) -> HttpError {
    if err.is_connect() {
        if find_in_chain::<rustls::pki_types::InvalidDnsNameError>(err).is_some() {
            warn!(err = %format!("{err:?}"), "outbound TLS protocol error");
            return HttpError::TlsProtocolError;
        }
        if let Some(io) = find_in_chain::<std::io::Error>(err) {
            let facts = unwrap_io_error(io);
            if facts.tls {
                warn!(err = %format!("{err:?}"), "outbound TLS protocol error");
                return HttpError::TlsProtocolError;
            }
            return match facts.kind {
                std::io::ErrorKind::AddrNotAvailable => dns_error("address not available"),
                std::io::ErrorKind::TimedOut => HttpError::ConnectionTimeout,
                _ if facts.resolver => dns_error("address not available"),
                _ => HttpError::ConnectionRefused,
            };
        }
        return HttpError::ConnectionRefused;
    }

    if let Some(hyper_err) = find_in_chain::<hyper::Error>(err)
        && hyper_err.is_timeout()
    {
        return HttpError::ConnectionTimeout;
    }
    warn!(err = %format!("{err:?}"), "outbound HTTP protocol error");
    HttpError::HttpProtocolError
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
                    outbound_http: http,
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
        let http = crate::host::quota::QuotaLimits::default().outbound_http;
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

    /// Writes a self-signed certificate and its key, as an operator mounts a
    /// client credential.
    fn write_identity(dir: &std::path::Path, stem: &str) -> ClientIdentity {
        let issued = rcgen::generate_simple_self_signed(vec!["client".to_string()])
            .expect("failed to generate test certificate");
        let cert_path = dir.join(format!("{stem}.crt"));
        let key_path = dir.join(format!("{stem}.key"));
        std::fs::write(&cert_path, issued.cert.pem()).unwrap();
        std::fs::write(&key_path, issued.signing_key.serialize_pem()).unwrap();
        ClientIdentity::CertificatePem {
            cert_path,
            key_path,
        }
    }

    #[test]
    fn no_client_identity_presents_nothing() {
        let config = ClientTlsOptions::default()
            .build()
            .expect("default options build");
        assert!(!config.client_auth_cert_resolver.has_certs());
    }

    #[test]
    fn a_client_identity_is_presented() {
        let dir = tempfile::tempdir().unwrap();
        let opts = ClientTlsOptions {
            client_identity: Some(write_identity(dir.path(), "id")),
            ..Default::default()
        };
        let config = opts.build().expect("a matching pair builds");
        assert!(config.client_auth_cert_resolver.has_certs());
    }

    /// A crossed pair fails at build time rather than on every handshake,
    /// where it would look like the peer rejecting the host.
    #[test]
    fn a_mismatched_client_identity_fails_to_build() {
        let dir = tempfile::tempdir().unwrap();
        // Cross the pair: the certificate from one, the key from another.
        let (
            ClientIdentity::CertificatePem { cert_path, .. },
            ClientIdentity::CertificatePem { key_path, .. },
        ) = (
            write_identity(dir.path(), "first"),
            write_identity(dir.path(), "second"),
        );
        let opts = ClientTlsOptions {
            client_identity: Some(ClientIdentity::CertificatePem {
                cert_path,
                key_path,
            }),
            ..Default::default()
        };
        assert!(opts.build().is_err());
    }

    #[test]
    fn a_missing_client_identity_fails_to_build() {
        let opts = ClientTlsOptions {
            client_identity: Some(ClientIdentity::CertificatePem {
                cert_path: PathBuf::from("/definitely/not/a/real/client.crt"),
                key_path: PathBuf::from("/definitely/not/a/real/client.key"),
            }),
            ..Default::default()
        };
        assert!(opts.build().is_err());
    }

    /// A resolver keyed on the workload, which is what per-workload identity
    /// needs: two workloads must not collapse onto one configuration.
    #[derive(Debug)]
    struct PerWorkload {
        a: Arc<rustls::ClientConfig>,
        b: Arc<rustls::ClientConfig>,
        host: Arc<rustls::ClientConfig>,
    }

    impl ClientTlsConfigResolver for PerWorkload {
        fn config_for(&self, workload_id: &str) -> Arc<rustls::ClientConfig> {
            match workload_id {
                "a" => Arc::clone(&self.a),
                "b" => Arc::clone(&self.b),
                _ => Arc::clone(&self.host),
            }
        }

        fn host_config(&self) -> Arc<rustls::ClientConfig> {
            Arc::clone(&self.host)
        }
    }

    fn per_workload_resolver() -> Arc<PerWorkload> {
        let dir = tempfile::tempdir().unwrap();
        let build = |stem: &str| {
            ClientTlsOptions {
                client_identity: Some(write_identity(dir.path(), stem)),
                ..Default::default()
            }
            .build()
            .unwrap()
        };
        Arc::new(PerWorkload {
            a: build("a"),
            b: build("b"),
            host: default_client_tls_config(),
        })
    }

    #[test]
    fn an_arc_config_answers_every_workload_the_same_way() {
        let config = default_client_tls_config();
        let resolver: Arc<dyn ClientTlsConfigResolver> = Arc::new(Arc::clone(&config));
        assert!(Arc::ptr_eq(&resolver.config_for("a"), &config));
        assert!(Arc::ptr_eq(&resolver.config_for("b"), &config));
        assert!(Arc::ptr_eq(&resolver.host_config(), &config));
    }

    #[test]
    fn each_workloads_client_gets_its_own_configuration() {
        let resolver = per_workload_resolver();
        let clients = WorkloadClients::with_tls_config_resolver(
            Arc::clone(&resolver) as _,
            test_quotas(4, 16),
        );

        let a = clients.client("a").tls_config();
        let b = clients.client("b").tls_config();
        assert!(Arc::ptr_eq(&a, &resolver.a));
        assert!(Arc::ptr_eq(&b, &resolver.b));
        assert!(!Arc::ptr_eq(&a, &b));
    }

    /// `tls_config` is host-wide by contract: a caller reaching for it must
    /// not silently receive one workload's identity.
    #[test]
    fn the_host_configuration_is_not_a_workloads() {
        let resolver = per_workload_resolver();
        let clients = WorkloadClients::with_tls_config_resolver(
            Arc::clone(&resolver) as _,
            test_quotas(4, 16),
        );
        let host = clients.tls_config();
        assert!(Arc::ptr_eq(&host, &resolver.host));
        assert!(!Arc::ptr_eq(&host, &resolver.a));
    }

    /// Rebuilding a cache for new quotas must not flatten per-workload
    /// identities onto the host-wide configuration.
    #[test]
    fn a_rebuilt_cache_keeps_its_resolver() {
        let resolver = per_workload_resolver();
        let clients = WorkloadClients::with_tls_config_resolver(
            Arc::clone(&resolver) as _,
            test_quotas(4, 16),
        );
        let rebuilt = WorkloadClients::with_tls_config_resolver(
            clients.tls_config_resolver(),
            test_quotas(8, 32),
        );
        assert!(Arc::ptr_eq(&rebuilt.client("a").tls_config(), &resolver.a));
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
            ..Default::default()
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
            ..Default::default()
        };
        assert!(opts.build().is_err());
    }

    #[test]
    fn extra_only_without_bundles_is_rejected() {
        let opts = ClientTlsOptions {
            roots: TrustRoots::ExtraOnly,
            extra_ca_paths: vec![],
            ..Default::default()
        };
        let err = opts.build().expect_err("an empty trust store must fail");
        assert!(err.to_string().contains("trust store is empty"), "{err}");
    }

    #[test]
    fn webpki_only_builds() {
        let opts = ClientTlsOptions {
            roots: TrustRoots::Webpki,
            extra_ca_paths: vec![],
            ..Default::default()
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

    fn grpc_request(uri: &str) -> hyper::Request<WasiBody> {
        hyper::Request::builder()
            .uri(uri)
            .method(hyper::Method::POST)
            .header(hyper::header::CONTENT_TYPE, "application/grpc")
            .body(WasiBody::default())
            .unwrap()
    }

    fn test_options() -> Option<RequestOptions> {
        Some(RequestOptions {
            connect_timeout: Some(Duration::from_secs(5)),
            first_byte_timeout: Some(Duration::from_secs(5)),
            between_bytes_timeout: Some(Duration::from_secs(5)),
        })
    }

    fn request(uri: &str) -> hyper::Request<WasiBody> {
        hyper::Request::builder()
            .uri(uri)
            .body(WasiBody::default())
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
            let (response, _io) = client
                .send_request(request(&format!("http://{addr}/")), test_options())
                .await
                .expect("request should succeed");
            assert_eq!(response.status(), 200);
            // Drain the body so the connection is returned to the pool.
            let _ = response.into_body().collect().await;
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
                let (response, _io) = client
                    .send_request(request(&uri), test_options())
                    .await
                    .expect("request should succeed");
                assert_eq!(response.status(), 200);
                // Drain the body so the connection is returned to the pool.
                let _ = response.into_body().collect().await;
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

    /// A guest may set any `between-bytes-timeout`, including zero, and
    /// `tokio::time::interval` panics on a zero period — so the response-body
    /// wrapper must clamp it rather than take the host down.
    #[tokio::test]
    async fn zero_between_bytes_timeout_is_not_a_panic() {
        let (addr, _conns) = spawn_counting_server().await;
        let client = PooledClient::new(default_client_tls_config());
        let options = RequestOptions {
            connect_timeout: None,
            first_byte_timeout: None,
            between_bytes_timeout: Some(Duration::ZERO),
        };

        let (response, _io) = client
            .send_request(request(&format!("http://{addr}/")), Some(options))
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

        let (response, _io) = client
            .send_request(
                request(&format!("http://{addr}/")),
                Some(RequestOptions {
                    connect_timeout: Some(Duration::from_millis(150)),
                    first_byte_timeout: Some(Duration::from_secs(5)),
                    between_bytes_timeout: Some(Duration::from_secs(5)),
                }),
            )
            .await
            .expect("a slow head must not be charged against the connect deadline");
        assert_eq!(response.status(), 200);
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
        let connect_timeout = TEST_PERMIT_WAIT / 5;
        let (addr, _conns) = spawn_counting_server_with_delay(TEST_PERMIT_WAIT * 2).await;
        let clients = WorkloadClients::with_quotas(default_client_tls_config(), test_quotas(1, 1));
        let uri = format!("http://{addr}/");

        // Occupy the only permit for the duration of the slow request.
        let busy = clients.client("workload-a");
        let busy_uri = uri.clone();
        let busy = tokio::spawn(async move {
            let (response, _io) = busy
                .send_request(request(&busy_uri), test_options())
                .await
                .expect("the first request should get the only connection");
            let _ = response.into_body().collect().await;
        });
        tokio::time::sleep(Duration::from_millis(150)).await;

        let started = tokio::time::Instant::now();
        let err = clients
            .client("workload-b")
            .send_request(
                request(&uri),
                Some(RequestOptions {
                    connect_timeout: Some(connect_timeout),
                    first_byte_timeout: Some(Duration::from_secs(30)),
                    between_bytes_timeout: Some(Duration::from_secs(30)),
                }),
            )
            .await
            .err()
            .expect("no slot is available, so the connect deadline must expire");
        let elapsed = started.elapsed();

        assert!(
            matches!(err, HttpError::ConnectionTimeout),
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
        let (response, _io) = client
            .send_request(request(&uri), test_options())
            .await
            .expect("request should succeed");
        let _ = response.into_body().collect().await;
        drop(client);
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 1);

        clients.invalidate("workload-a");

        let client = clients.client("workload-a");
        let (response, _io) = client
            .send_request(request(&uri), test_options())
            .await
            .expect("request should succeed");
        let _ = response.into_body().collect().await;
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
            let (response, _io) = client
                .send_grpc_request(grpc_request(&uri), test_options())
                .await
                .expect("gRPC request should succeed");
            assert_eq!(response.status(), 200);
            let _ = response.into_body().collect().await;
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
        let (grpc_addr, _grpc_conns) = spawn_counting_h2c_server().await;
        let (http_addr, http_conns) = spawn_counting_server().await;
        let clients =
            WorkloadClients::with_quotas(default_client_tls_config(), test_quotas(1, 100));
        let client = clients.client("workload-a");

        // One gRPC connection, left idle in the h2 pool still holding the
        // workload's only permit.
        let (response, _io) = client
            .send_grpc_request(
                grpc_request(&format!("http://{grpc_addr}/svc.Test/Call")),
                test_options(),
            )
            .await
            .expect("gRPC request should succeed");
        let _ = response.into_body().collect().await;

        let err = client
            .send_request(
                request(&format!("http://{http_addr}/")),
                Some(RequestOptions {
                    connect_timeout: Some(TEST_PERMIT_WAIT / 5),
                    first_byte_timeout: Some(Duration::from_secs(30)),
                    between_bytes_timeout: Some(Duration::from_secs(30)),
                }),
            )
            .await
            .err()
            .expect("the gRPC connection holds the workload's only permit");
        assert!(
            matches!(err, HttpError::ConnectionTimeout),
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
            if let Ok((response, _io)) = busy_client
                .send_request(request(&busy_uri), test_options())
                .await
            {
                let _ = response.into_body().collect().await;
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;

        let quota = clients.quotas().for_guest("workload-a");
        assert_eq!(
            quota.outbound_http_available(),
            0,
            "the in-flight connection should hold the workload's only slot"
        );

        clients.invalidate("workload-a");
        let _rebuilt = clients.client("workload-a");
        let rebuilt_quota = clients.quotas().for_guest("workload-a");
        assert_eq!(
            rebuilt_quota.outbound_http_available(),
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
        let client = PooledClient::new(default_client_tls_config());

        // RFC 6761 reserves `.invalid`: resolution always fails.
        //
        // The generous `connect_timeout` is load-bearing. [`send_head`] races
        // the send against that deadline, so a resolver slow to answer — a
        // GitHub macOS runner walking its search domains has needed more than
        // the 5s [`test_options`] hands out — lets the timeout arm win and the
        // assertion below reads `ConnectionTimeout`. What is pinned here is how
        // a resolver failure classifies, not how fast the ambient resolver
        // reports one.
        let err = client
            .send_request(
                request("http://definitely-not-a-real-host.invalid/"),
                Some(RequestOptions {
                    connect_timeout: Some(Duration::from_secs(60)),
                    // The rest stay short: a head that hangs should fail this
                    // test by name rather than park until the suite times out.
                    ..test_options().unwrap_or_default()
                }),
            )
            .await
            .err()
            .expect("resolution must fail");
        assert!(
            matches!(err, HttpError::DnsError { .. }),
            "expected DnsError, got {err:?}"
        );

        // A loopback port nothing listens on: TCP connect is refused.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let err = client
            .send_request(
                request(&format!("http://127.0.0.1:{port}/")),
                test_options(),
            )
            .await
            .err()
            .expect("connect must be refused");
        assert!(
            matches!(err, HttpError::ConnectionRefused),
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

    /// The response body must keep streaming after the head on the pooled
    /// path, including when the request-error future has already been driven
    /// to completion — connection lifetime belongs to the pool, not to that
    /// future.
    #[tokio::test]
    async fn body_streams_after_head_on_pooled_connection() {
        let port = delayed_body_server(Duration::from_millis(300)).await;
        let client = PooledClient::new(default_client_tls_config());
        let (response, io) = client
            .send_request(request(&format!("http://127.0.0.1:{port}/")), None)
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

    /// The request-error future carries a failed upload to the guest, and a
    /// body dropped unread resolves as success.
    #[tokio::test]
    async fn upload_probe_reports_the_body_outcome() {
        struct FailingBody;
        impl hyper::body::Body for FailingBody {
            type Data = Bytes;
            type Error = HttpError;
            fn poll_frame(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, HttpError>>> {
                std::task::Poll::Ready(Some(Err(HttpError::HttpProtocolError)))
            }
        }

        // The error is moved, so the guest's own future gets it and whoever is
        // reading the body — hyper, or a co-located callee on the local path —
        // gets told only that it failed, as a network peer would be.
        let (probe, outcome) = UploadProbe::new(FailingBody.boxed_unsync());
        let seen_by_reader = BodyExt::collect(probe).await.err();
        assert!(
            matches!(seen_by_reader, Some(HttpError::InternalError(Some(ref m))) if m == "request body failed"),
            "the body's reader must see the failure marker, got {seen_by_reader:?}"
        );
        assert!(matches!(
            Box::into_pin(upload_io(outcome)).await,
            Err(HttpError::HttpProtocolError)
        ));

        let empty = http_body_util::Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed_unsync();
        let (probe, outcome) = UploadProbe::new(empty);
        drop(probe);
        assert!(Box::into_pin(upload_io(outcome)).await.is_ok());
    }

    /// A server that hangs up mid-body must surface as an error on the
    /// response body, not as a silently truncated success. On the pooled path
    /// this guarantee is enforced by hyper's content-length check surfacing
    /// through `TimedBody`'s error mapping — pin it here (ported from the
    /// per-request transport's test suite).
    #[tokio::test]
    async fn truncated_body_surfaces_as_body_error() {
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
            .send_request(request(&format!("http://127.0.0.1:{port}/")), None)
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
        // The truncation itself, not merely "some hyper error": a body that ends
        // early carries an `UnexpectedEof`. wasmtime turns this into
        // `HttpProtocolError` for the guest.
        let HttpError::Hyper(hyper_err) = &err else {
            panic!("expected a hyper error, got {err:?}");
        };
        let truncated = std::error::Error::source(hyper_err)
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .map(std::io::Error::kind);
        assert_eq!(
            truncated,
            Some(std::io::ErrorKind::UnexpectedEof),
            "expected a truncated body, got {err:?}"
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
                let (response, _io) = client
                    .send_request(request(&uri), test_options())
                    .await
                    .expect("request should succeed despite waiting for a connection");
                assert_eq!(response.status(), 200);
                // Drain the body so the connection is returned to the pool.
                let _ = response.into_body().collect().await;
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
                let (response, _io) = client
                    .send_request(request(&uri), test_options())
                    .await
                    .expect("request should succeed despite waiting for a connection");
                assert_eq!(response.status(), 200);
                let _ = response.into_body().collect().await;
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
        let (response, _io) = client_b
            .send_request(request(&uri), test_options())
            .await
            .expect("workload B should connect once A's permits are released");
        assert_eq!(response.status(), 200);
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
        let (port, ca_pem) = private_ca_tls_server().await;
        let dir = tempfile::tempdir().unwrap();
        let ca_path = dir.path().join("ca.pem");
        std::fs::write(&ca_path, ca_pem).unwrap();

        // Without the CA: the handshake must fail with a TLS error.
        let default_client = PooledClient::new(default_client_tls_config());
        let err = default_client
            .send_request(
                request(&format!("https://127.0.0.1:{port}/")),
                test_options(),
            )
            .await
            .err()
            .expect("untrusted CA must fail");
        assert!(
            matches!(err, HttpError::TlsProtocolError),
            "expected TlsProtocolError, got {err:?}"
        );

        // With the CA: the same request must succeed.
        let tls = ClientTlsOptions {
            roots: TrustRoots::ExtraOnly,
            extra_ca_paths: vec![ca_path],
            ..Default::default()
        }
        .build()
        .unwrap();
        let client = PooledClient::new(tls);
        let (response, _io) = client
            .send_request(
                request(&format!("https://127.0.0.1:{port}/")),
                test_options(),
            )
            .await
            .expect("request with the private CA trusted should succeed");
        assert_eq!(response.status(), 200);
    }
}
