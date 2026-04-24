//! Integration test for TLS-enabled HTTP server
//!
//! Verifies that HTTPS requests are correctly handled when the HTTP server
//! is configured with TLS via `HttpServer::new_with_tls()`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, path::Path, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{LocalResources, WorkloadStartRequest},
};

mod common;
use common::{component_workload_request, http_counter_host_interfaces, start_host_with_tls};

const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");

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

fn http_counter_request(host_header: &str) -> WorkloadStartRequest {
    component_workload_request(
        "http-counter.wasm",
        "http-counter-tls",
        HTTP_COUNTER_WASM,
        LocalResources {
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
        http_counter_host_interfaces(host_header),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_https_request_succeeds() -> Result<()> {
    let (_dir, cert_path, key_path) = generate_test_certs()?;
    let (addr, host) = start_host_with_tls(&cert_path, &key_path).await?;

    let req = http_counter_request("foo");
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
