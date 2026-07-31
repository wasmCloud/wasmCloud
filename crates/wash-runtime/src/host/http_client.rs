//! Outbound HTTP transport with configurable TLS trust roots.
//!
//! wasmtime's `default_send_request` transport verifies outbound TLS against a
//! fixed, compiled-in copy of the webpki (Mozilla) roots, with no way to reach
//! hosts behind a corporate or otherwise private CA. This module replaces that
//! transport for the P2 and P3 egress paths: same per-request connection
//! behavior and error mapping, but the TLS trust roots are built from
//! [`ClientTlsOptions`] — webpki roots plus the platform's native certificate
//! store (which honours `SSL_CERT_FILE`/`SSL_CERT_DIR`) plus any explicitly
//! configured PEM bundles.

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
use wasmtime_wasi_http::io::TokioIo;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::hyper_request_error;
use wasmtime_wasi_http::p2::types::{IncomingResponse, OutgoingRequestConfig};

use crate::host::http_p3::{P3Body, P3RequestErrorFuture};

/// Trust-root options for outbound HTTPS from components.
///
/// The resulting root store always contains the compiled-in webpki (Mozilla)
/// roots; the options add to (or trim) that base.
#[derive(Debug, Clone, Default)]
pub struct ClientTlsOptions {
    /// Additional PEM CA bundle files to trust (each file may contain one or
    /// more certificates). Use this to reach hosts behind a corporate or
    /// otherwise private CA.
    pub extra_ca_paths: Vec<PathBuf>,
    /// Skip loading the platform's native root store. The native store honours
    /// the conventional `SSL_CERT_FILE`/`SSL_CERT_DIR` environment overrides,
    /// so leaving this `false` (the default) lets operators inject CAs without
    /// any wash-specific configuration.
    pub disable_native_certs: bool,
}

impl ClientTlsOptions {
    /// Build a rustls client configuration from these options.
    ///
    /// Fails only when an entry in `extra_ca_paths` cannot be read or contains
    /// no usable certificate; problems loading individual native-store
    /// certificates are logged and skipped.
    pub fn build(&self) -> anyhow::Result<Arc<rustls::ClientConfig>> {
        crate::init_crypto();
        let mut roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };

        if !self.disable_native_certs {
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

        Ok(Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        ))
    }
}

/// Process-wide default outbound TLS configuration (webpki + native roots),
/// built once on first use.
pub fn default_client_tls_config() -> Arc<rustls::ClientConfig> {
    static DEFAULT: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    DEFAULT
        .get_or_init(|| {
            // With no extra_ca_paths this cannot fail, but fall back to
            // webpki-only roots rather than panicking if it ever does.
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

/// The authority (`host:port`) for a request, defaulting the port from the
/// scheme like wasmtime's default transport does.
fn request_authority<B>(request: &hyper::Request<B>, use_tls: bool) -> Option<String> {
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
fn to_origin_form<B>(request: &mut hyper::Request<B>) {
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
    use wasmtime_wasi_http::p2::bindings::http::types::{DnsErrorPayload, ErrorCode};

    fn dns_error(rcode: &str) -> ErrorCode {
        ErrorCode::DnsError(DnsErrorPayload {
            rcode: Some(rcode.to_string()),
            info_code: Some(0),
        })
    }

    if !request.headers().contains_key(hyper::header::HOST)
        && let Some(authority) = request.uri().authority()
        && let Ok(value) = hyper::header::HeaderValue::from_str(authority.as_str())
    {
        request.headers_mut().insert(hyper::header::HOST, value);
    }

    let authority =
        request_authority(&request, use_tls).ok_or(ErrorCode::HttpRequestUriInvalid)?;

    let tcp_stream = timeout(connect_timeout, TcpStream::connect(&authority))
        .await
        .map_err(|_| ErrorCode::ConnectionTimeout)?
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrNotAvailable => dns_error("address not available"),
            _ if e
                .to_string()
                .starts_with("failed to lookup address information") =>
            {
                dns_error("address not available")
            }
            _ => ErrorCode::ConnectionRefused,
        })?;

    let (mut sender, worker) = if use_tls {
        let connector = tokio_rustls::TlsConnector::from(tls);
        let domain = tls_server_name(&authority).ok_or_else(|| {
            warn!(authority = %authority, "invalid TLS server name");
            dns_error("invalid dns name")
        })?;
        let stream = connector.connect(domain, tcp_stream).await.map_err(|e| {
            warn!("tls protocol error: {e:?}");
            ErrorCode::TlsProtocolError
        })?;
        let (sender, conn) = timeout(connect_timeout, http1::handshake(TokioIo::new(stream)))
            .await
            .map_err(|_| ErrorCode::ConnectionTimeout)?
            .map_err(hyper_request_error)?;
        let worker = wasmtime_wasi::runtime::spawn(async move {
            if let Err(e) = conn.await {
                warn!(err = %e, "dropping outbound connection error");
            }
        });
        (sender, worker)
    } else {
        let (sender, conn) = timeout(connect_timeout, http1::handshake(TokioIo::new(tcp_stream)))
            .await
            .map_err(|_| ErrorCode::ConnectionTimeout)?
            .map_err(hyper_request_error)?;
        let worker = wasmtime_wasi::runtime::spawn(async move {
            if let Err(e) = conn.await {
                warn!(err = %e, "dropping outbound connection error");
            }
        });
        (sender, worker)
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
/// resolves `Ok(())` (hyper reports body errors through the response body's
/// own stream).
pub(crate) async fn send_request_p3(
    tls: Arc<rustls::ClientConfig>,
    mut request: hyper::Request<P3Body>,
    options: Option<wasmtime_wasi_http::p3::RequestOptions>,
) -> crate::host::http_p3::P3SendResult {
    use wasmtime_wasi_http::p3::bindings::http::types::{DnsErrorPayload, ErrorCode};

    fn dns_error(rcode: &str) -> ErrorCode {
        ErrorCode::DnsError(DnsErrorPayload {
            rcode: Some(rcode.to_string()),
            info_code: Some(0),
        })
    }

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

    if !request.headers().contains_key(hyper::header::HOST)
        && let Some(authority) = request.uri().authority()
        && let Ok(value) = hyper::header::HeaderValue::from_str(authority.as_str())
    {
        request.headers_mut().insert(hyper::header::HOST, value);
    }

    let authority =
        request_authority(&request, use_tls).ok_or(ErrorCode::HttpRequestUriInvalid)?;

    let tcp_stream = timeout(connect_timeout, TcpStream::connect(&authority))
        .await
        .map_err(|_| ErrorCode::ConnectionTimeout)?
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrNotAvailable => dns_error("address not available"),
            _ if e
                .to_string()
                .starts_with("failed to lookup address information") =>
            {
                dns_error("address not available")
            }
            _ => ErrorCode::ConnectionRefused,
        })?;

    let (mut sender, _worker) = if use_tls {
        let connector = tokio_rustls::TlsConnector::from(tls);
        let domain = tls_server_name(&authority).ok_or_else(|| {
            warn!(authority = %authority, "invalid TLS server name");
            dns_error("invalid dns name")
        })?;
        let stream = connector.connect(domain, tcp_stream).await.map_err(|e| {
            warn!("tls protocol error: {e:?}");
            ErrorCode::TlsProtocolError
        })?;
        let (sender, conn) = timeout(connect_timeout, http1::handshake(TokioIo::new(stream)))
            .await
            .map_err(|_| ErrorCode::ConnectionTimeout)?
            .map_err(ErrorCode::from_hyper_request_error)?;
        let worker = wasmtime_wasi::runtime::spawn(async move {
            if let Err(e) = conn.await {
                warn!(err = %e, "dropping outbound connection error");
            }
        });
        (sender, worker)
    } else {
        let (sender, conn) = timeout(connect_timeout, http1::handshake(TokioIo::new(tcp_stream)))
            .await
            .map_err(|_| ErrorCode::ConnectionTimeout)?
            .map_err(ErrorCode::from_hyper_request_error)?;
        let worker = wasmtime_wasi::runtime::spawn(async move {
            if let Err(e) = conn.await {
                warn!(err = %e, "dropping outbound connection error");
            }
        });
        (sender, worker)
    };

    to_origin_form(&mut request);

    let resp = timeout(first_byte_timeout, sender.send_request(request))
        .await
        .map_err(|_| ErrorCode::ConnectionReadTimeout)?
        .map_err(ErrorCode::from_hyper_request_error)?
        .map(|body| {
            let mut interval = tokio::time::interval(between_bytes_timeout);
            interval.reset();
            crate::host::http::TimedBody {
                inner: Some(body),
                interval,
            }
            .boxed_unsync()
        });

    // The connection worker is moved into this future so it stays alive until
    // the guest is done with the request, matching wasmtime's default
    // transport. Body errors reach the guest through the response body stream.
    let io: P3RequestErrorFuture = Box::new(async move {
        let _worker = _worker;
        Ok(())
    });
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
            extra_ca_paths: vec![path],
            disable_native_certs: true,
        };
        opts.build().expect("PEM CA bundle should load");
    }

    #[test]
    fn ca_bundle_without_certificates_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.pem");
        std::fs::write(&path, "not a certificate\n").unwrap();
        let opts = ClientTlsOptions {
            extra_ca_paths: vec![path],
            disable_native_certs: true,
        };
        assert!(opts.build().is_err());
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

    /// HTTPS to a server behind a private CA must work once that CA is added
    /// via `extra_ca_paths`, and must keep failing with a TLS error without it.
    #[tokio::test]
    async fn extra_ca_enables_https_to_private_ca_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

        crate::init_crypto();
        let certified_key =
            rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
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

        let dir = tempfile::tempdir().unwrap();
        let ca_path = dir.path().join("ca.pem");
        std::fs::write(&ca_path, certified_key.cert.pem()).unwrap();

        // Without the CA: the handshake must fail with a TLS error.
        let err = send_request_p2(
            default_client_tls_config(),
            p2_request(&format!("https://localhost:{port}/")),
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
            extra_ca_paths: vec![ca_path],
            disable_native_certs: true,
        }
        .build()
        .unwrap();
        let response = send_request_p2(
            tls,
            p2_request(&format!("https://localhost:{port}/")),
            p2_config(true),
        )
        .await
        .expect("request with the private CA trusted should succeed");
        assert_eq!(response.resp.status(), 200);
    }
}
