use std::{collections::HashMap, sync::Arc};

use super::NatsBlobstore;

use anyhow::Context as _;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;

async fn plugin() -> anyhow::Result<NatsBlobstore> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let (_, client) = tokio::try_join!(
        crate::nats_credentials::tests::accept_connect(&listener, "nonce"),
        async {
            anyhow::Ok(async_nats::connect(format!("nats://{}", listener.local_addr()?)).await?)
        },
    )?;
    Ok(NatsBlobstore::new(&client))
}

#[tokio::test]
async fn absent_credentials_preserve_the_host_connection() -> anyhow::Result<()> {
    let plugin = plugin().await?;
    for config in [
        HashMap::new(),
        HashMap::from([
            ("servers".into(), "nats://unused:4222".into()),
            ("tls-ca".into(), "/unused/ca.crt".into()),
            ("tls-first".into(), "true".into()),
        ]),
    ] {
        assert!(plugin.connect_binding(&config).await?.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn incomplete_client_identity_is_refused() -> anyhow::Result<()> {
    let plugin = plugin().await?;
    for key in ["tls-cert", "TLS_KEY"] {
        let config = HashMap::from([
            ("servers".into(), "nats://unused:4222".into()),
            (key.into(), "/tenant.pem".into()),
        ]);
        let error = plugin.connect_binding(&config).await.unwrap_err();
        assert!(
            error.to_string().contains("must be set together"),
            "{error:#}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn blank_identity_is_refused() -> anyhow::Result<()> {
    let plugin = plugin().await?;
    for blank in [
        vec![("token", "")],
        vec![("tls-cert", " "), ("tls-key", "")],
    ] {
        let mut config = HashMap::from([("servers".to_string(), "nats://unused:4222".to_string())]);
        config.extend(blank.into_iter().map(|(k, v)| (k.into(), v.into())));
        let error = plugin.connect_binding(&config).await.unwrap_err();
        assert!(error.to_string().contains("empty values"), "{error:#}");
    }
    Ok(())
}

#[tokio::test]
async fn websocket_servers_are_refused() -> anyhow::Result<()> {
    let plugin = plugin().await?;
    for scheme in ["ws", "wss"] {
        let config = HashMap::from([
            ("servers".into(), format!("{scheme}://127.0.0.1:4222")),
            ("tls-cert".into(), "/tenant.crt".into()),
            ("tls-key".into(), "/tenant.key".into()),
        ]);
        let error = plugin.connect_binding(&config).await.unwrap_err();
        assert!(error.to_string().contains("WebSocket"), "{error:#}");
    }
    Ok(())
}

#[tokio::test]
async fn client_identity_requires_tls() -> anyhow::Result<()> {
    crate::init_crypto();
    let plugin = plugin().await?;
    let dir = tempfile::tempdir()?;
    let tenant = rcgen::generate_simple_self_signed(vec!["tenant".into()])?;
    let cert_path = dir.path().join("tenant.crt");
    let key_path = dir.path().join("tenant.key");
    std::fs::write(&cert_path, tenant.cert.pem())?;
    std::fs::write(&key_path, tenant.signing_key.serialize_pem())?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let config = HashMap::from([
        (
            "servers".into(),
            format!("nats://{}", listener.local_addr()?),
        ),
        ("tls-cert".into(), cert_path.display().to_string()),
        ("tls-key".into(), key_path.display().to_string()),
    ]);
    let plaintext_server = async {
        let (mut socket, _) = listener.accept().await?;
        socket
            .write_all(b"INFO {\"max_payload\":1048576}\r\n")
            .await?;
        let mut first = [0u8; 1];
        socket.read_exact(&mut first).await?;
        anyhow::Ok(first[0])
    };
    let (first, binding) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(plaintext_server, plugin.connect_binding(&config))
    })
    .await
    .context("binding neither connected nor failed")?;
    // 0x16 opens a TLS handshake record; a plaintext client sends `CONNECT`.
    assert_eq!(first?, 0x16);
    assert!(binding.is_err());
    Ok(())
}

#[tokio::test]
async fn certificate_only_binding_presents_its_own_identity() -> anyhow::Result<()> {
    crate::init_crypto();
    let plugin = plugin().await?;
    let dir = tempfile::tempdir()?;
    let mut ca_params = CertificateParams::new(Vec::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate()?)?;
    let server_key = KeyPair::generate()?;
    let mut server_params = CertificateParams::new(vec!["localhost".into()])?;
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server = server_params.signed_by(&server_key, &ca)?;
    let tenant_key = KeyPair::generate()?;
    let mut tenant_params = CertificateParams::new(vec!["tenant".into()])?;
    tenant_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let tenant = tenant_params.signed_by(&tenant_key, &ca)?;
    let ca_path = dir.path().join("ca.crt");
    let cert_path = dir.path().join("tenant.crt");
    let key_path = dir.path().join("tenant.key");
    std::fs::write(&ca_path, ca.pem())?;
    std::fs::write(&cert_path, tenant.pem())?;
    std::fs::write(&key_path, tenant_key.serialize_pem())?;

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca.der().clone())?;
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()?;
    let server_config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        )?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let config = HashMap::from([
        (
            "servers".into(),
            format!("tls://localhost:{}", listener.local_addr()?.port()),
        ),
        ("tls-ca".into(), ca_path.display().to_string()),
        ("TLS_CERT".into(), cert_path.display().to_string()),
        ("TLS_KEY".into(), key_path.display().to_string()),
    ]);
    let handshake = async {
        let (mut socket, _) = listener.accept().await?;
        socket
            .write_all(b"INFO {\"tls_required\":true,\"max_payload\":1048576}\r\n")
            .await?;
        let stream = acceptor.accept(socket).await?;
        assert_eq!(
            stream
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|certs| certs.first()),
            Some(tenant.der()),
        );
        let mut stream = BufReader::new(stream);
        loop {
            let mut line = String::new();
            anyhow::ensure!(stream.read_line(&mut line).await? > 0, "closed before PING");
            if line.starts_with("PING") {
                stream.get_mut().write_all(b"PONG\r\n").await?;
                break;
            }
        }
        anyhow::Ok(stream)
    };
    let (_stream, binding) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::try_join!(handshake, plugin.connect_binding(&config))
    })
    .await
    .context("certificate-only binding did not open its own connection")??;
    assert!(binding.is_some());
    Ok(())
}
