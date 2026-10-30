//! Which CAs an OCI pull trusts, end to end against in-process registries.
//!
//! Each registry answers every request with a 404, so a pull never succeeds.
//! What the tests read is whether the request got through the TLS handshake,
//! which the server counts.

// `std::env::set_var` is unsafe on edition 2024. This binary holds a single
// test on a current-thread runtime, and each change lands between pulls, when
// no other thread is reading the environment.
#![cfg(feature = "oci")]
#![allow(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use wash_runtime::oci::{OciConfig, OciPullPolicy, pull_component, set_extra_ca_certificates};

#[path = "common/oci_ca.rs"]
mod ca;
#[path = "common/oci_registry.rs"]
mod cache_registry;
use ca::Ca;

/// A registry on loopback that counts the requests it read.
struct Registry {
    port: u16,
    reached: Arc<AtomicUsize>,
}

impl Registry {
    /// Serve HTTPS with a certificate `ca` signs, or plain HTTP when `None`.
    async fn start(ca: Option<&Ca>) -> Self {
        let acceptor = ca.map(|ca| TlsAcceptor::from(Arc::new(ca.server_config())));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let reached = Arc::new(AtomicUsize::new(0));
        let counter = reached.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                let counter = counter.clone();
                tokio::spawn(async move {
                    match acceptor {
                        Some(acceptor) => {
                            if let Ok(stream) = acceptor.accept(stream).await {
                                answer_404(stream, &counter).await;
                            }
                        }
                        None => answer_404(stream, &counter).await,
                    }
                });
            }
        });
        Self { port, reached }
    }

    fn reference(&self) -> String {
        format!("localhost:{}/test/component:v1", self.port)
    }

    fn reached(&self) -> usize {
        self.reached.load(Ordering::SeqCst)
    }
}

async fn answer_404(mut stream: impl AsyncRead + AsyncWrite + Unpin, reached: &AtomicUsize) {
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let Some(chunk) = buf.get(..n) else { return };
                request.extend_from_slice(chunk);
            }
        }
    }
    reached.fetch_add(1, Ordering::SeqCst);
    let body = r#"{"errors":[{"code":"MANIFEST_UNKNOWN","message":"manifest unknown"}]}"#;
    let response = format!(
        "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Pull from `registry` and report whether the request passed TLS.
///
/// Explicit credentials keep the pull off the docker credential helper.
async fn pull_reaches(registry: &Registry, insecure: bool) -> Result<bool, anyhow::Error> {
    pull_reaches_with_policy(registry, insecure, OciPullPolicy::Always).await
}

async fn pull_reaches_with_policy(
    registry: &Registry,
    insecure: bool,
    policy: OciPullPolicy,
) -> Result<bool, anyhow::Error> {
    let before = registry.reached();
    let config = OciConfig {
        credentials: Some(("user".into(), "pass".into())),
        insecure,
        ..Default::default()
    };
    let err = tokio::time::timeout(
        Duration::from_secs(30),
        pull_component(&registry.reference(), config, policy),
    )
    .await
    .context("pull timed out")?
    .expect_err("a registry that answers 404 cannot serve a component");
    if registry.reached() > before {
        return Ok(true);
    }
    Err(err)
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

fn set_env(key: &str, value: &Path) {
    unsafe { std::env::set_var(key, value) }
}

fn clear_env() {
    unsafe {
        std::env::remove_var("SSL_CERT_FILE");
        std::env::remove_var("SSL_CERT_DIR");
    }
}

fn assert_untrusted(outcome: Result<bool>, case: &str) {
    let err = outcome.expect_err(case);
    assert!(
        format!("{err:?}").contains("invalid peer certificate"),
        "{case}: expected a certificate error, got {err:?}"
    );
}

#[tokio::test]
async fn oci_pulls_trust_ssl_cert_env_and_ca_paths() -> Result<()> {
    wash_runtime::init_crypto();
    clear_env();

    let dir = TempDir::new()?;
    let ca_a = Ca::new("oci-test-ca-a");
    let ca_b = Ca::new("oci-test-ca-b");
    let registry_a = Registry::start(Some(&ca_a)).await;
    let registry_b = Registry::start(Some(&ca_b)).await;
    let bundle = dir.path().join("bundle.pem");

    assert_untrusted(
        pull_reaches(&registry_a, false).await,
        "a private CA nothing names is not trusted",
    );

    write(&bundle, &ca_a.pem());
    set_env("SSL_CERT_FILE", &bundle);
    assert!(
        pull_reaches(&registry_a, false).await?,
        "SSL_CERT_FILE's CA is trusted"
    );

    // Correct PEM framing around bytes that are not a certificate. The CA
    // after it must still be trusted, and the client keep its configuration.
    write(
        &bundle,
        &format!(
            "-----BEGIN CERTIFICATE-----\nbm90IGEgY2VydGlmaWNhdGU=\n-----END CERTIFICATE-----\n{}",
            ca_a.pem()
        ),
    );
    assert!(
        pull_reaches(&registry_a, false).await?,
        "an unusable entry in SSL_CERT_FILE is skipped, not fatal"
    );

    // Rotation: the bundle is rewritten in place and the next pull sees it.
    write(&bundle, &ca_b.pem());
    assert!(
        pull_reaches(&registry_b, false).await?,
        "a CA rotated into SSL_CERT_FILE is trusted without a restart"
    );
    assert_untrusted(
        pull_reaches(&registry_a, false).await,
        "a CA rotated out of SSL_CERT_FILE is no longer trusted",
    );

    // Populate a pinned disk entry under CA A, then remove that CA. Both the
    // completed-result cache and the disk cache must honor the new snapshot.
    let cached_registry =
        cache_registry::Registry::start_with_tls(Some(ca_a.server_config())).await;
    let cache = TempDir::new()?;
    let config = OciConfig {
        credentials: Some(("user".into(), "secret".into())),
        cache_dir: Some(cache.path().to_path_buf()),
        timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    let cached_reference = cached_registry.reference();
    write(&bundle, &ca_a.pem());
    pull_component(
        &cached_reference,
        config.clone(),
        OciPullPolicy::IfNotPresent,
    )
    .await?;
    let fetched = cached_registry.requests.load(Ordering::SeqCst);
    write(&bundle, &ca_b.pem());
    assert!(
        pull_component(&cached_reference, config.clone(), OciPullPolicy::Never)
            .await
            .is_err()
    );
    assert_eq!(
        cached_registry.requests.load(Ordering::SeqCst),
        fetched,
        "Never stays offline on a trust-scope miss"
    );
    assert_untrusted(
        pull_component(
            &cached_reference,
            config.clone(),
            OciPullPolicy::IfNotPresent,
        )
        .await
        .map(|_| true),
        "a pinned disk entry cannot outlive the CA that authorized its fetch",
    );
    write(&bundle, &ca_a.pem());
    pull_component(&cached_reference, config, OciPullPolicy::Never).await?;
    assert_eq!(
        cached_registry.requests.load(Ordering::SeqCst),
        fetched,
        "Never reuses the matching trust scope offline"
    );

    // An `insecure` pull keeps speaking plain HTTP with extra roots set.
    let plain = Registry::start(None).await;
    assert!(
        pull_reaches(&plain, true).await?,
        "insecure pulls reach an HTTP registry"
    );

    clear_env();
    let cert_dir = dir.path().join("certs");
    std::fs::create_dir(&cert_dir)?;
    write(&cert_dir.join("ca-a.pem"), &ca_a.pem());
    set_env("SSL_CERT_DIR", &cert_dir);
    assert!(
        pull_reaches(&registry_a, false).await?,
        "SSL_CERT_DIR's CA is trusted"
    );

    // `--ca-path` last: the store it writes can be set once per process.
    clear_env();
    assert_untrusted(
        pull_reaches_with_policy(&registry_b, false, OciPullPolicy::IfNotPresent).await,
        "cache a failure before installing the extra CA",
    );
    let ca_path = dir.path().join("ca-path.pem");
    write(&ca_path, &ca_b.pem());
    set_extra_ca_certificates(&[ca_path])?;
    assert!(
        pull_reaches_with_policy(&registry_b, false, OciPullPolicy::IfNotPresent).await?,
        "a --ca-path CA is trusted immediately, including through the result cache"
    );
    assert_untrusted(
        pull_reaches(&registry_a, false).await,
        "--ca-path trusts only the CA it names",
    );

    Ok(())
}
