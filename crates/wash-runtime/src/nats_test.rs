use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Complete one handshake advertising `advertised` and keep the connection open.
pub(crate) async fn accept_advertising(
    listener: &TcpListener,
    advertised: SocketAddr,
) -> anyhow::Result<TcpStream> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut socket, _) = listener.accept().await?;
        socket
            .write_all(
                format!("INFO {{\"max_payload\":1048576,\"connect_urls\":[\"{advertised}\"]}}\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut reader = BufReader::new(&mut socket);
        loop {
            let mut line = String::new();
            anyhow::ensure!(reader.read_line(&mut line).await? > 0, "closed before PING");
            if line.starts_with("PING") {
                break;
            }
        }
        socket.write_all(b"PONG\r\n").await?;
        Ok(socket)
    })
    .await?
}

/// A NATS server's TLS identity for `localhost`, and the CA file that trusts it.
pub(crate) struct TlsServer {
    pub(crate) ca: std::path::PathBuf,
    acceptor: tokio_rustls::TlsAcceptor,
    _dir: tempfile::TempDir,
}

impl TlsServer {
    pub(crate) fn new() -> anyhow::Result<Self> {
        use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};

        crate::init_crypto();
        let mut ca_params = CertificateParams::new(Vec::new())?;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate()?)?;
        let key = KeyPair::generate()?;
        let cert = CertificateParams::new(vec!["localhost".into()])?.signed_by(&key, &ca)?;
        let dir = tempfile::tempdir()?;
        let ca_path = dir.path().join("ca.crt");
        std::fs::write(&ca_path, ca.pem())?;
        let config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )?;
        Ok(Self {
            ca: ca_path,
            acceptor: tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config)),
            _dir: dir,
        })
    }

    /// Advertise `advertised` in the plaintext INFO, complete the TLS handshake
    /// and one PING, and keep the connection open.
    pub(crate) async fn accept_advertising(
        &self,
        listener: &TcpListener,
        advertised: &str,
    ) -> anyhow::Result<tokio_rustls::server::TlsStream<TcpStream>> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (mut socket, _) = listener.accept().await?;
            socket
                .write_all(
                    format!(
                        "INFO {{\"tls_required\":true,\"max_payload\":1048576,\
                         \"connect_urls\":[\"{advertised}\"]}}\r\n"
                    )
                    .as_bytes(),
                )
                .await?;
            let mut stream = BufReader::new(self.acceptor.accept(socket).await?);
            loop {
                let mut line = String::new();
                anyhow::ensure!(stream.read_line(&mut line).await? > 0, "closed before PING");
                if line.starts_with("PING") {
                    break;
                }
            }
            stream.get_mut().write_all(b"PONG\r\n").await?;
            Ok(stream.into_inner())
        })
        .await?
    }
}
