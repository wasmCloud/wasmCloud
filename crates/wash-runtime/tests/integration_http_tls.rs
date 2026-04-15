//! Integration test for TLS-enabled HTTP server
//!
//! Verifies that HTTPS requests are correctly handled when the HTTP server
//! is configured with TLS via `HttpServer::new_with_tls()`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::{
        wasi_blobstore::InMemoryBlobstore, wasi_config::DynamicConfig,
        wasi_keyvalue::InMemoryKeyValue, wasi_logging::TracingLogger,
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");

/// Ensure the rustls CryptoProvider is installed exactly once per process.
fn init_crypto() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .expect("failed to install crypto provider");
    });
}

/// Generate a self-signed certificate and private key for `localhost`,
/// write them to a temp directory, and return the paths.
///
/// The returned `TempDir` must be held alive for the duration of the test.
fn generate_test_certs() -> Result<(tempfile::TempDir, std::path::PathBuf, std::path::PathBuf)> {
    let dir = tempfile::TempDir::new()?;

    let certified_key = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .context("failed to generate self-signed certificate")?;

    let cert_path = dir.path().join("server.crt");
    let key_path = dir.path().join("server.key");

    std::fs::write(&cert_path, certified_key.cert.pem())?;
    std::fs::write(&key_path, certified_key.signing_key.serialize_pem())?;

    Ok((dir, cert_path, key_path))
}

/// Build a `reqwest::Client` that trusts the given self-signed certificate.
fn build_tls_client(cert_path: &Path) -> Result<reqwest::Client> {
    let cert_pem = std::fs::read(cert_path)?;
    let cert = reqwest::Certificate::from_pem(&cert_pem)?;
    reqwest::Client::builder()
        .add_root_certificate(cert)
        .build()
        .context("failed to build TLS client")
}

/// Start a host with TLS-enabled HTTP server and the standard set of plugins.
async fn start_host_with_tls(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new_with_tls(
        DevRouter::default(),
        "127.0.0.1:0".parse()?,
        cert_path,
        key_path,
        None,
    )
    .await?;
    let bound_addr = http_server.addr();

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(InMemoryBlobstore::new(None)))?
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host))
}

/// Standard host interfaces for the http-counter component.
fn http_counter_host_interfaces(http_host_config: &str) -> Vec<WitInterface> {
    vec![
        WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config: {
                let mut config = HashMap::new();
                config.insert("host".to_string(), http_host_config.to_string());
                config
            },
            name: None,
        },
        WitInterface {
            namespace: "wasi".to_string(),
            package: "blobstore".to_string(),
            interfaces: [
                "blobstore".to_string(),
                "container".to_string(),
                "types".to_string(),
            ]
            .into_iter()
            .collect(),
            version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
            config: HashMap::new(),
            name: None,
        },
        WitInterface {
            namespace: "wasi".to_string(),
            package: "keyvalue".to_string(),
            interfaces: ["store".to_string(), "atomics".to_string()]
                .into_iter()
                .collect(),
            version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
            config: HashMap::new(),
            name: None,
        },
        WitInterface {
            namespace: "wasi".to_string(),
            package: "logging".to_string(),
            interfaces: ["logging".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
            config: HashMap::new(),
            name: None,
        },
        WitInterface {
            namespace: "wasi".to_string(),
            package: "config".to_string(),
            interfaces: ["store".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.0-rc.1").unwrap()),
            config: HashMap::new(),
            name: None,
        },
    ]
}

fn http_counter_workload_request(http_host_config: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "http-counter-tls".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-counter.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_COUNTER_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 256,
                    cpu_limit: 1,
                    config: HashMap::from([
                        ("test_key".to_string(), "test_value".to_string()),
                        ("counter_enabled".to_string(), "true".to_string()),
                    ]),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: Default::default(),
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: http_counter_host_interfaces(http_host_config),
            volumes: vec![],
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_https_request_succeeds() -> Result<()> {
    init_crypto();

    let (_dir, cert_path, key_path) = generate_test_certs()?;
    let (addr, host) = start_host_with_tls(&cert_path, &key_path).await?;

    let req = http_counter_workload_request("foo");
    host.workload_start(req)
        .await
        .context("Failed to start http-counter workload")?;

    let client = build_tls_client(&cert_path)?;

    // First request should initialize counter to 1
    let first_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("https://localhost:{}/", addr.port()))
            .header("HOST", "foo")
            .send(),
    )
    .await
    .context("First request timed out")?
    .context("Failed to make first HTTPS request")?;

    assert!(
        first_response.status().is_success(),
        "First HTTPS request failed with status {}",
        first_response.status(),
    );

    let first_text = first_response.text().await?;
    assert_eq!(
        first_text.trim(),
        "1",
        "First request should return counter value of 1"
    );

    // Second request should increment counter
    let second_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("https://localhost:{}/", addr.port()))
            .header("HOST", "foo")
            .send(),
    )
    .await
    .context("Second request timed out")?
    .context("Failed to make second HTTPS request")?;

    assert!(second_response.status().is_success());

    let second_count: u64 = second_response
        .text()
        .await?
        .trim()
        .parse()
        .expect("Response should be a valid number");
    assert!(
        second_count >= 1,
        "Second request should return counter >= 1, got {second_count}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_https_rejects_plain_http() -> Result<()> {
    init_crypto();

    let (_dir, cert_path, key_path) = generate_test_certs()?;
    let (addr, _host) = start_host_with_tls(&cert_path, &key_path).await?;

    // A plain HTTP client should not be able to talk to an HTTPS server
    let client = reqwest::Client::new();
    let result = timeout(
        Duration::from_secs(5),
        client
            .get(format!("http://localhost:{}/", addr.port()))
            .send(),
    )
    .await;

    match result {
        Err(_) => {}     // Timeout — expected
        Ok(Err(_)) => {} // Connection error — expected
        Ok(Ok(resp)) => {
            panic!(
                "Plain HTTP request to HTTPS server should fail, got status {}",
                resp.status()
            );
        }
    }

    Ok(())
}
