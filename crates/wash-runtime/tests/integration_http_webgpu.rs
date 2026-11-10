//! Integration tests for HTTP and webgpu plugins working together.
//!
//! Most run on the noop backend, which completes GPU work instantly. The
//! runaway-shader test needs a real GPU and runs only with
//! `WASH_TEST_WEBGPU_HARDWARE=1`.
#![cfg_attr(coverage_nightly, recursion_limit = "256")]
#![cfg(all(
    feature = "wasi-webgpu",
    not(target_os = "windows"),
    not(target_arch = "s390x")
))]

use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, Ingress},
    },
    plugin::wasi_webgpu::{WebGpu, WebGpuBackend},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_WEBGPU_WASM: &[u8] = include_bytes!("wasm/http_webgpu.wasm");

/// Starts a host with `webgpu` and the fixture workload, returning the
/// host (to keep it alive) and the ingress address.
async fn start(webgpu: WebGpu) -> Result<(impl HostApi, SocketAddr)> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(webgpu))?
        .build()?
        .start()
        .await
        .context("Failed to start host")?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "test-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-webgpu-component".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_WEBGPU_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 256,
                    cpu_limit: 1,
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: Default::default(),
                    allowed_ip_name_lookups: Default::default(),
                    allowed_host_loopback_ports: Default::default(),
                },
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 1,
                ..Default::default()
            }],
            host_interfaces: vec![
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.2.2").map_err(|e| anyhow::anyhow!(e))?),
                    config: {
                        let mut config = wash_runtime::wit::Config::new();
                        config.insert("host".to_string(), "foo".to_string());
                        config
                    },
                    name: None,
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "webgpu".to_string(),
                    interfaces: ["webgpu".to_string()].into_iter().collect(),
                    version: Some(
                        semver::Version::parse("0.3.0-rc.2").map_err(|e| anyhow::anyhow!(e))?,
                    ),
                    config: Default::default(),
                    name: None,
                },
            ],
            volumes: vec![],
        },
    };
    host.workload_start(req)
        .await
        .context("Failed to start workload")?;
    Ok((host, addr))
}

async fn get(addr: SocketAddr, path: &str, limit: Duration) -> Result<String> {
    let response = timeout(
        limit,
        reqwest::Client::new()
            .post(format!("http://{addr}{path}"))
            .send(),
    )
    .await
    .context("HTTP request timed out")?
    .context("Failed to make HTTP request")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("Failed to read response body")?;
    anyhow::ensure!(status.is_success(), "{path}: {status}: {body}");
    Ok(body)
}

#[tokio::test]
async fn test_http_webgpu_integration() -> Result<()> {
    let (_host, addr) = start(WebGpu::new(WebGpuBackend::Noop)).await?;

    let body = get(addr, "/1,2,3,4", Duration::from_secs(5)).await?;
    // The noop backend leaves the numbers as they are.
    assert_eq!(body, "results: [1, 2, 3, 4]");
    Ok(())
}

#[tokio::test]
async fn test_http_webgpu_destroy_resolves_lost() -> Result<()> {
    let (_host, addr) = start(WebGpu::new(WebGpuBackend::Noop)).await?;

    let body = get(addr, "/lost", Duration::from_secs(5)).await?;
    assert_eq!(body, "lost: destroyed: device destroyed");
    Ok(())
}

#[tokio::test]
async fn test_http_webgpu_zero_timeout_waits() -> Result<()> {
    let webgpu = WebGpu::new(WebGpuBackend::Noop).with_wait_timeout(Some(Duration::ZERO));
    let (_host, addr) = start(webgpu).await?;

    let body = get(addr, "/1,2,3,4", Duration::from_secs(5)).await?;
    assert_eq!(body, "results: [1, 2, 3, 4]");
    Ok(())
}

fn hardware() -> bool {
    let set = std::env::var_os("WASH_TEST_WEBGPU_HARDWARE").is_some();
    if !set {
        eprintln!("skipping: set WASH_TEST_WEBGPU_HARDWARE=1 to run on a real GPU");
    }
    set
}

/// A shader that outlives the wait timeout fails the guest's `map-async`,
/// loses its device, and leaves the guest able to keep calling in.
#[tokio::test]
async fn test_http_webgpu_runaway_shader_loses_device() -> Result<()> {
    if !hardware() {
        return Ok(());
    }
    let wait = Duration::from_millis(500);
    let webgpu = WebGpu::new(WebGpuBackend::All).with_wait_timeout(Some(wait));
    let (_host, addr) = start(webgpu).await?;

    // Seconds of GPU time, so the shader ends on its own after the test.
    // The message shows the timeout cut the map short; the request limit
    // bounds the rest, including device and shader setup.
    let body = get(addr, "/runaway/2000000000", Duration::from_secs(10)).await?;

    let mut lines = body.lines();
    assert_eq!(
        lines.next(),
        Some("map: OperationError: GPU work did not finish within 500ms"),
        "{body}"
    );
    assert_eq!(
        lines.next(),
        Some("lost: unknown: GPU work did not finish within 500ms"),
        "{body}"
    );
    assert_eq!(lines.next(), Some("after: ok"), "{body}");
    Ok(())
}

/// Destroying a device aborts a map still waiting on its work.
#[tokio::test]
async fn test_http_webgpu_destroy_aborts_pending_map() -> Result<()> {
    if !hardware() {
        return Ok(());
    }
    let webgpu = WebGpu::new(WebGpuBackend::All).with_wait_timeout(Some(Duration::from_secs(5)));
    let (_host, addr) = start(webgpu).await?;

    // Only `destroy` aborts the map this way; the 5s timeout would report
    // an OperationError instead.
    let body = get(
        addr,
        "/destroy-during-map/2000000000",
        Duration::from_secs(10),
    )
    .await?;
    assert_eq!(body, "map: AbortError: device destroyed");
    Ok(())
}
