//! `wash oci pull` suggests `--ca-path` only for a certificate a CA could fix,
//! checked against the real TLS errors rather than hand-written strings.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use wash::cli::CliContext;
use wash::cli::oci::{PullCommand, RegistryArgs};

const HINT: &str = "--ca-path <bundle.pem>";

fn ca(name: &str) -> CertifiedIssuer<'static, KeyPair> {
    let now = SystemTime::now();
    let mut params = CertificateParams::new(Vec::new()).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = (now - Duration::from_secs(172_800)).into();
    params.not_after = (now + Duration::from_secs(172_800)).into();
    CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
}

/// Serve TLS on loopback with a `localhost` certificate `ca` signs, valid
/// until `not_after`. Every handshake is dropped once it completes.
async fn start_registry(ca: &CertifiedIssuer<'static, KeyPair>, not_after: SystemTime) -> u16 {
    let now = SystemTime::now();
    let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = (now - Duration::from_secs(86_400)).into();
    params.not_after = not_after.into();
    let key = KeyPair::generate().unwrap();
    let cert = params.signed_by(&key, ca).unwrap();
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone(), ca.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut stream) = acceptor.accept(stream).await {
                    let _ = stream.shutdown().await;
                }
            });
        }
    });
    port
}

/// Run `wash oci pull` against `port` and return its error, formatted the way
/// the CLI prints it.
async fn pull_error(port: u16, ca_paths: Vec<std::path::PathBuf>) -> Result<String> {
    let temp = TempDir::new()?;
    let ctx = CliContext::builder()
        .non_interactive(true)
        .project_dir(temp.path().to_path_buf())
        .build()
        .await
        .context("failed to create CLI context")?;
    let pull = PullCommand {
        reference: format!("localhost:{port}/test/component:v1"),
        component_path: temp.path().join("component.wasm"),
        registry: RegistryArgs {
            user: Some("user".into()),
            password: Some("pass".into()),
            ca_paths,
            ..Default::default()
        },
    };
    let err = tokio::time::timeout(Duration::from_secs(30), pull.handle(&ctx))
        .await
        .context("pull timed out")?
        .expect_err("a registry that drops every connection cannot serve a component");
    Ok(format!("{err:?}"))
}

#[tokio::test]
async fn untrusted_ca_suggests_ca_path() -> Result<()> {
    wash_runtime::init_crypto();
    let port = start_registry(
        &ca("hint-untrusted-ca"),
        SystemTime::now() + Duration::from_secs(86_400),
    )
    .await;

    let err = pull_error(port, Vec::new()).await?;
    assert!(err.contains(HINT), "expected the --ca-path hint: {err}");
    Ok(())
}

/// An expired certificate from a trusted CA fails verification too, but no CA
/// bundle can fix it.
#[tokio::test]
async fn expired_certificate_does_not_suggest_ca_path() -> Result<()> {
    wash_runtime::init_crypto();
    let ca = ca("hint-trusted-ca");
    let dir = TempDir::new()?;
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca.pem())?;
    let port = start_registry(&ca, SystemTime::now() - Duration::from_secs(3_600)).await;

    let err = pull_error(port, vec![ca_path]).await?;
    assert!(
        err.contains("invalid peer certificate"),
        "expected a certificate error: {err}"
    );
    assert!(
        !err.contains(HINT),
        "an expired certificate got the hint: {err}"
    );
    Ok(())
}
