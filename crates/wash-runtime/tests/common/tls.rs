//! Shared TLS test helpers: in-process rustls echo server, custom
//! [`TlsProvider`] that trusts a self-signed certificate, and engine
//! builders wired up with that provider.

#![cfg(feature = "wasi-tls")]
#![allow(dead_code)]

use anyhow::{Context, Result, bail};
use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    pin::Pin,
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ClientConfig, RootCertStore, ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
    },
};
use wasmtime_wasi_tls::{Error as TlsError, TlsProvider, TlsStream, TlsTransport};

use wash_runtime::engine::Engine;

/// Returns the outbound IPv4 via a UDP routing-table lookup (no packets sent).
/// Returns `Ok(None)` if no non-loopback IPv4 interface is available.
pub fn detect_non_loopback_ipv4() -> Result<Option<Ipv4Addr>> {
    let sock = UdpSocket::bind("0.0.0.0:0").context("failed to bind discovery UDP socket")?;
    if sock.connect("192.0.2.1:80").is_err() {
        return Ok(None);
    }
    match sock.local_addr() {
        Ok(SocketAddr::V4(v4)) if !v4.ip().is_loopback() && !v4.ip().is_unspecified() => {
            Ok(Some(*v4.ip()))
        }
        _ => Ok(None),
    }
}

/// Generate a self-signed certificate for `localhost` and return the rustls
/// `ServerConfig` together with the DER-encoded certificate bytes (used to
/// build the client trust store).
fn server_tls_config() -> Result<(ServerConfig, Vec<u8>)> {
    let certified_key = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .context("failed to generate self-signed certificate")?;

    let cert_der_bytes = certified_key.cert.der().to_vec();
    let cert_der = CertificateDer::from(cert_der_bytes.clone());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        certified_key.signing_key.serialize_der(),
    ));

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .context("failed to build ServerConfig")?;

    Ok((config, cert_der_bytes))
}

pub struct EchoServer {
    pub addr: SocketAddr,
    pub cert_der: Vec<u8>,
    /// Fires with the bytes received before the server writes `PONG\r\n`.
    pub ping_rx: oneshot::Receiver<Vec<u8>>,
}

/// Start a TLS echo server on a non-loopback IPv4 interface (required so
/// wash-runtime's loopback interception doesn't swallow the connection).
pub async fn start_tls_echo_server() -> Result<EchoServer> {
    let Some(ip) = detect_non_loopback_ipv4()? else {
        bail!(
            "no non-loopback IPv4 interface available on this host; \
             cannot run a TLS round-trip test against the real network stack"
        );
    };

    let (server_config, cert_der) = server_tls_config()?;
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(ip), 0))
        .await
        .with_context(|| format!("failed to bind echo server on {ip}:0"))?;
    let addr = listener.local_addr()?;

    let (ping_tx, ping_rx) = oneshot::channel::<Vec<u8>>();

    tokio::spawn(async move {
        let mut ping_tx = Some(ping_tx);
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            let acceptor = acceptor.clone();
            let ping_slot = ping_tx.take();
            tokio::spawn(async move {
                let mut tls_stream = match acceptor.accept(stream).await {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let mut buf = vec![0u8; 256];
                let mut received = Vec::new();
                loop {
                    match tls_stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            received.extend_from_slice(&buf[..n]);
                            if received.windows(2).any(|w| w == b"\r\n") {
                                break;
                            }
                        }
                        Err(_) => return,
                    }
                }
                if let Some(tx) = ping_slot {
                    let _ = tx.send(received);
                }
                let _ = tls_stream.write_all(b"PONG\r\n").await;
                let _ = tls_stream.flush().await;
            });
        }
    });

    Ok(EchoServer {
        addr,
        cert_der,
        ping_rx,
    })
}

/// A [`TlsProvider`] that uses a custom `rustls` `ClientConfig`.
///
/// Used in tests to inject a self-signed certificate as a trusted root so the
/// guest component can connect to the in-process echo server.
struct TestTlsProvider {
    client_config: Arc<ClientConfig>,
}

/// Newtype wrapper so we can implement the foreign `TlsStream` marker trait on
/// a `tokio_rustls::client::TlsStream` without violating the orphan rule.
struct ClientTlsStream(tokio_rustls::client::TlsStream<Box<dyn TlsTransport>>);

impl tokio::io::AsyncRead for ClientTlsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for ClientTlsStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl Unpin for ClientTlsStream {}
impl TlsStream for ClientTlsStream {}

impl TlsProvider for TestTlsProvider {
    fn connect(
        &self,
        server_name: String,
        transport: Box<dyn TlsTransport>,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn TlsStream>, TlsError>> + Send>> {
        let config = Arc::clone(&self.client_config);
        Box::pin(async move {
            let domain = ServerName::try_from(server_name.clone())
                .map_err(|_| TlsError::msg("invalid server name"))?;
            let stream = tokio_rustls::TlsConnector::from(config)
                .connect(domain, transport)
                .await
                .map_err(|e| TlsError::msg(e.to_string()))?;
            Ok(Box::new(ClientTlsStream(stream)) as Box<dyn TlsStream>)
        })
    }
}

/// Build a [`ClientConfig`] that trusts the given DER-encoded certificate bytes.
fn client_config_with_cert(cert_der: &[u8]) -> Result<ClientConfig> {
    let mut root_store = RootCertStore::empty();
    root_store
        .add(CertificateDer::from(cert_der.to_vec()))
        .context("failed to add certificate to root store")?;
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Ok(config)
}

/// Wrap `cert_der` into an `Arc<dyn TlsProvider>` that trusts only that cert.
fn test_tls_provider(cert_der: &[u8]) -> Result<Arc<dyn TlsProvider>> {
    let client_config = Arc::new(client_config_with_cert(cert_der)?);
    Ok(Arc::new(TestTlsProvider { client_config }) as Arc<dyn TlsProvider>)
}

/// Build an engine with P3 and a custom TLS provider that trusts `cert_der`.
#[cfg(feature = "wasip3")]
pub fn engine_with_p3_and_tls(cert_der: &[u8]) -> Result<Engine> {
    Engine::builder()
        .with_wasip3(true)
        .with_tls_provider(test_tls_provider(cert_der)?)
        .build()
        .context("failed to build P3+TLS engine")
}

/// Install the default `aws-lc-rs` rustls crypto provider exactly once.
///
/// Required when both `aws-lc-rs` and `ring` features are enabled (no
/// unambiguous default). Safe to call repeatedly.
pub fn install_default_crypto_provider() {
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
}
