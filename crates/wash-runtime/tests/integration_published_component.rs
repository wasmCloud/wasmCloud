#![cfg(all(
    feature = "oci",
    feature = "wasi-blobstore",
    feature = "wasm_component_model_implements"
))]
//! A component published before this host was built keeps serving on it.
//!
//! Every fixture is compiled alongside the host, on the workspace's toolchain,
//! so only a published binary shows a host upgrade breaking what registries
//! already hold. Pulls from ghcr.io, so the test is `#[ignore]`d.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::oci::{OciConfig, OciPullPolicy, pull_component};
use wash_runtime::plugin::wasi_blobstore::{InMemoryProvider, MultiplexedAsyncBlobstore};
use wash_runtime::plugin::wasmcloud_secrets::WasmcloudSecrets;
use wash_runtime::types::{Component, Workload, WorkloadStartRequest, WorkloadState};
use wash_runtime::wit::WitInterface;

mod common;
use common::{http_incoming_handler_interface, secrets_interface};

/// `ghcr.io/wasmcloud/components/oci-registry:0.2.0`, built on wit-bindgen 0.60.
/// By digest, so it stays that build whatever the tag comes to point at.
const IMAGE: &str = "ghcr.io/wasmcloud/components/oci-registry@sha256:5f2229855c6826056642068a94f7b88ac5c23217f9a332c82c8c80665a71ea70";
const HOST: &str = "published";
const USERNAME: &str = "published-test";
const PASSWORD: &str = "published-test-password";
const BLOB: &[u8] = b"published before this host was built";
/// `sha256` of [`BLOB`].
const BLOB_DIGEST: &str = "sha256:9f75af320373d6f618704f66c239199116a90e60a1ff5fcf669752396bc76e9e";
/// What the host logs when a guest traps, whatever the trap.
const TRAP: &str = "wasm trap";
/// How long a trap that follows the last response has to reach the log.
const SETTLE: Duration = Duration::from_millis(500);

/// Everything the host logs at WARN and above. Cloning shares the buffer.
#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl LogCapture {
    fn lines_containing(&self, needle: &str) -> Vec<String> {
        let buf = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&buf)
            .lines()
            .filter(|line| line.contains(needle))
            .map(str::to_owned)
            .collect()
    }
}

impl io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogCapture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Print to the test output as `RUST_LOG` asks, and record WARN and above
/// whatever it asks. Global, because the host also logs off the test's thread.
fn init_tracing() -> LogCapture {
    let capture = LogCapture::default();
    let console = tracing_subscriber::fmt::layer()
        .with_test_writer()
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()));
    let captured = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(capture.clone())
        .with_filter(LevelFilter::WARN);
    tracing_subscriber::registry()
        .with(console)
        .with(captured)
        .try_init()
        .expect("the only global subscriber in this test binary");
    capture
}

/// The async `wasmcloud:blobstore` import, unnamed: the in-memory default.
fn blobstore_interface() -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "blobstore".to_string(),
        interfaces: ["blobstore", "container", "types"]
            .map(str::to_string)
            .into_iter()
            .collect(),
        version: Some(semver::Version::new(0, 1, 0)),
        config: HashMap::new(),
        name: None,
    }
}

/// Serve the published registry, which reads the credentials it enforces
/// through `wasmcloud:secrets`.
async fn serve(wasm: Vec<u8>, digest: String) -> Result<(SocketAddr, impl HostApi)> {
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(Engine::builder().build()?)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(
            MultiplexedAsyncBlobstore::new().with_provider(Arc::new(InMemoryProvider)),
        ))?
        .with_plugin(Arc::new(WasmcloudSecrets::new()))?
        .build()?
        .start()
        .await
        .context("failed to start host")?;

    let credentials = HashMap::from([
        ("registry-username".to_string(), USERNAME.to_string()),
        ("registry-password".to_string(), PASSWORD.to_string()),
    ]);
    let started = host
        .workload_start(WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "test".to_string(),
                name: HOST.to_string(),
                annotations: HashMap::new(),
                service: None,
                components: vec![Component {
                    name: "oci-registry".to_string(),
                    digest: Some(digest),
                    bytes: wasm.into(),
                    ..Default::default()
                }],
                host_interfaces: vec![
                    http_incoming_handler_interface(HOST, None),
                    blobstore_interface(),
                    secrets_interface(credentials),
                ],
                volumes: vec![],
            },
        })
        .await
        .context("workload_start call failed")?;
    assert_eq!(
        started.workload_status.workload_state,
        WorkloadState::Running,
        "{}",
        started.workload_status.message
    );
    Ok((addr, host))
}

#[tokio::test]
#[ignore = "pulls a published component from ghcr.io (network)"]
async fn a_published_component_serves_on_this_host() -> Result<()> {
    let logs = init_tracing();
    let (wasm, digest) = pull_component(IMAGE, OciConfig::default(), OciPullPolicy::IfNotPresent)
        .await
        .context("failed to pull the published component")?;
    let (addr, _host) = serve(wasm, digest).await?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let url = |path: &str| format!("http://{addr}{path}");
    let request = |method: reqwest::Method, url: String| {
        client
            .request(method, url)
            .header("HOST", HOST)
            .basic_auth(USERNAME, Some(PASSWORD))
    };

    let api = request(reqwest::Method::GET, url("/v2/")).send().await?;
    assert_eq!(api.status().as_u16(), 200, "GET /v2/");

    // Push a blob the way a registry client does: open an upload, then
    // complete it with the body. Both bodies stream through the blobstore.
    let opened = request(reqwest::Method::POST, url("/v2/published/blobs/uploads/"))
        .send()
        .await?;
    assert_eq!(opened.status().as_u16(), 202, "opening an upload");
    let location = opened
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .context("the upload has no Location header")?
        .to_string();
    let location = match location.starts_with('/') {
        true => url(&location),
        false => location,
    };
    let separator = match location.contains('?') {
        true => '&',
        false => '?',
    };
    let completed = request(
        reqwest::Method::PUT,
        format!("{location}{separator}digest={BLOB_DIGEST}"),
    )
    .header("content-type", "application/octet-stream")
    .body(BLOB)
    .send()
    .await?;
    assert_eq!(completed.status().as_u16(), 201, "completing the upload");

    let pulled = request(
        reqwest::Method::GET,
        url(&format!("/v2/published/blobs/{BLOB_DIGEST}")),
    )
    .send()
    .await?;
    assert_eq!(pulled.status().as_u16(), 200, "pulling the blob");
    assert_eq!(pulled.bytes().await?.as_ref(), BLOB);

    // A trap can follow a response that already went out.
    tokio::time::sleep(SETTLE).await;
    let traps = logs.lines_containing(TRAP);
    assert!(
        traps.is_empty(),
        "every request succeeded, but the guest trapped:\n{}",
        traps.join("\n")
    );
    Ok(())
}
