//! Outbound HTTP transport with configurable TLS trust roots.
//!
//! wasmtime's `default_send_request` transport verifies outbound TLS against a
//! fixed, compiled-in copy of the webpki (Mozilla) roots, with no way to reach
//! hosts behind a corporate or otherwise private CA. This module replaces that
//! transport for the P2 and P3 egress paths: same per-request connection
//! behavior and error mapping, but the TLS trust roots are built from
//! [`ClientTlsOptions`] — a [`TrustRoots`] base (webpki and/or the platform's
//! native store, which honours `SSL_CERT_FILE`/`SSL_CERT_DIR`) with any
//! explicitly configured PEM bundles layered on top.
//!
//! Built from upstream: [`send_request_p2`] mirrors wasmtime's
//! `wasmtime_wasi_http::p2::default_send_request_handler` and
//! [`send_request_p3`] mirrors `wasmtime_wasi_http::p3::default_send_request`.
//! When upgrading wasmtime, diff this module against those to pick up upstream
//! fixes.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use http_body_util::BodyExt;
use hyper::client::conn::http1;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, warn};
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi_http::io::TokioIo;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::hyper_request_error;
use wasmtime_wasi_http::p2::types::{IncomingResponse, OutgoingRequestConfig};

use crate::host::http_p3::{P3Body, P3RequestErrorFuture};

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

/// Add a `Host` header from the request authority when none is present.
fn set_host_header<B>(request: &mut hyper::Request<B>) {
    if !request.headers().contains_key(hyper::header::HOST)
        && let Some(authority) = request.uri().authority()
        && let Ok(value) = hyper::header::HeaderValue::from_str(authority.as_str())
    {
        request.headers_mut().insert(hyper::header::HOST, value);
    }
}

/// Send a P2 outgoing request: wasmtime's `default_send_request_handler` with
/// the trust roots taken from `tls` instead of the compiled-in webpki bundle.
pub(crate) async fn send_request_p2(
    tls: Arc<rustls::ClientConfig>,
    mut request: hyper::Request<HyperOutgoingBody>,
    OutgoingRequestConfig {
        use_tls,
        connect_timeout,
        first_byte_timeout,
        between_bytes_timeout,
    }: OutgoingRequestConfig,
) -> Result<IncomingResponse, wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> {
    use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

    set_host_header(&mut request);
    let authority = request_authority(&request, use_tls).ok_or(ErrorCode::HttpRequestUriInvalid)?;
    let tcp_stream = connect_tcp(&authority, connect_timeout).await?;

    let (mut sender, worker) = if use_tls {
        let stream = connect_tls(tls, &authority, tcp_stream).await?;
        let (sender, conn) = timeout(connect_timeout, http1::handshake(TokioIo::new(stream)))
            .await
            .map_err(|_| ErrorCode::ConnectionTimeout)?
            .map_err(hyper_request_error)?;
        (sender, spawn_p2_conn_worker(conn))
    } else {
        let (sender, conn) = timeout(connect_timeout, http1::handshake(TokioIo::new(tcp_stream)))
            .await
            .map_err(|_| ErrorCode::ConnectionTimeout)?
            .map_err(hyper_request_error)?;
        (sender, spawn_p2_conn_worker(conn))
    };

    to_origin_form(&mut request);

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

/// Send a P3 outgoing request: wasmtime's `default_send_request` with the
/// trust roots taken from `tls` instead of the compiled-in webpki bundle.
///
/// Mirrors the structure of the P3 gRPC egress path in `host::http`: the
/// response body enforces the between-bytes timeout via
/// [`crate::host::http::TimedBody`], and the returned request-error future
/// drives the hyper connection, resolving with the connection's outcome once
/// it finishes.
pub(crate) async fn send_request_p3(
    tls: Arc<rustls::ClientConfig>,
    mut request: hyper::Request<P3Body>,
    options: Option<wasmtime_wasi_http::p3::RequestOptions>,
) -> crate::host::http_p3::P3SendResult {
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

    set_host_header(&mut request);
    let authority = request_authority(&request, use_tls).ok_or(ErrorCode::HttpRequestUriInvalid)?;
    let tcp_stream = connect_tcp(&authority, connect_timeout)
        .await
        .map_err(ErrorCode::from)?;

    let (mut sender, conn_worker) = if use_tls {
        let stream = connect_tls(tls, &authority, tcp_stream)
            .await
            .map_err(ErrorCode::from)?;
        let (sender, conn) = timeout(connect_timeout, http1::handshake(TokioIo::new(stream)))
            .await
            .map_err(|_| ErrorCode::ConnectionTimeout)?
            .map_err(ErrorCode::from_hyper_request_error)?;
        (sender, spawn_p3_conn_worker(conn))
    } else {
        let (sender, conn) = timeout(connect_timeout, http1::handshake(TokioIo::new(tcp_stream)))
            .await
            .map_err(|_| ErrorCode::ConnectionTimeout)?
            .map_err(ErrorCode::from_hyper_request_error)?;
        (sender, spawn_p3_conn_worker(conn))
    };

    to_origin_form(&mut request);

    let resp = timeout(first_byte_timeout, sender.send_request(request))
        .await
        .map_err(|_| ErrorCode::ConnectionReadTimeout)?
        .map_err(ErrorCode::from_hyper_request_error)?
        .map(|body| crate::host::http::TimedBody::new(body, between_bytes_timeout).boxed_unsync());

    // The connection driver *is* the request-error future. `spawn` hands back
    // an `AbortOnDropJoinHandle`, which aborts the connection task when
    // dropped, and wasmtime keeps this future alive for the response body's
    // lifetime only while it polls pending (see `p3::host::handler`).
    // Returning a future that is ready on the first poll would abort the
    // connection before the guest reads the body.
    let io: P3RequestErrorFuture = Box::new(conn_worker);
    Ok((resp, io))
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

    fn p3_request(uri: &str) -> hyper::Request<P3Body> {
        hyper::Request::builder()
            .uri(uri)
            .body(P3Body::new(
                http_body_util::Empty::new().map_err(|_: std::convert::Infallible| unreachable!()),
            ))
            .unwrap()
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
        let err = send_request_p2(
            default_client_tls_config(),
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
        let response = send_request_p2(
            tls,
            p2_request(&format!("https://127.0.0.1:{port}/")),
            p2_config(true),
        )
        .await
        .expect("request with the private CA trusted should succeed");
        assert_eq!(response.resp.status(), 200);
    }

    /// P3 egress must honour the configured trust roots too: same private-CA
    /// server as the P2 test, driven through `send_request_p3`.
    #[tokio::test]
    async fn p3_extra_ca_enables_https_to_private_ca_server() {
        use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

        let (port, ca_pem) = private_ca_tls_server().await;
        let dir = tempfile::tempdir().unwrap();
        let ca_path = dir.path().join("ca.pem");
        std::fs::write(&ca_path, ca_pem).unwrap();

        // Without the CA: the handshake must fail with a TLS error.
        let err = send_request_p3(
            default_client_tls_config(),
            p3_request(&format!("https://127.0.0.1:{port}/")),
            None,
        )
        .await
        .map(|_| ())
        .expect_err("untrusted CA must fail");
        let err = err.downcast().expect("a transport error, not a trap");
        assert!(
            matches!(err, ErrorCode::TlsProtocolError),
            "expected TlsProtocolError, got {err:?}"
        );

        // With the CA: the same request must succeed, and the body must arrive
        // while the request-error future drives the connection.
        let tls = ClientTlsOptions {
            roots: TrustRoots::ExtraOnly,
            extra_ca_paths: vec![ca_path],
        }
        .build()
        .unwrap();
        let (response, io) =
            send_request_p3(tls, p3_request(&format!("https://127.0.0.1:{port}/")), None)
                .await
                .expect("request with the private CA trusted should succeed");
        assert_eq!(response.status(), 200);
        let io = wasmtime_wasi::runtime::spawn(async move { Box::into_pin(io).await });
        let body = tokio::time::timeout(
            Duration::from_secs(3),
            BodyExt::collect(response.into_body()),
        )
        .await
        .expect("body read timed out")
        .expect("body read failed");
        assert_eq!(body.to_bytes().as_ref(), b"ok");
        drop(io);
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

    /// The request-error future returned by [`send_request_p3`] owns the hyper
    /// connection driver, so it must poll pending until the connection ends.
    /// wasmtime polls it once with a noop waker and only keeps it alive for the
    /// response body when it is pending, so a future that is ready immediately
    /// aborts the connection before the guest can read the body.
    #[tokio::test]
    async fn p3_request_error_future_keeps_connection_alive() {
        use core::task::{Context, Waker};

        let port = delayed_body_server(Duration::from_millis(300)).await;
        let (response, io) = send_request_p3(
            default_client_tls_config(),
            p3_request(&format!("http://127.0.0.1:{port}/")),
            None,
        )
        .await
        .expect("request should succeed");

        let mut io = Box::into_pin(io);
        assert!(
            io.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending(),
            "request-error future must stay pending so wasmtime ties it to the body"
        );

        // Drive it the way wasmtime does once it sees `Pending`.
        let io = wasmtime_wasi::runtime::spawn(io);
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

    /// A server that hangs up mid-body must surface as an error on the response
    /// body, not as a silently truncated success. hyper completes the
    /// connection driver cleanly here, so this is the channel the guest sees.
    #[tokio::test]
    async fn p3_truncated_body_surfaces_as_body_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            // Promise five bytes, then hang up without sending them.
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\n")
                .await;
            let _ = sock.flush().await;
            drop(sock);
        });

        let (response, io) = send_request_p3(
            default_client_tls_config(),
            p3_request(&format!("http://127.0.0.1:{port}/")),
            None,
        )
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
}
