//! Shared helpers for the wash-runtime benches. Each bench target compiles
//! this module separately and uses a subset of it, so unused items are
//! expected per-target.
#![allow(dead_code)]

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::Context as _;
use wash_runtime::{
    engine::Engine,
    host::{
        Host, HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    types::{
        Component, LocalResources, Service, Workload, WorkloadStartRequest, WorkloadState,
        WorkloadStopRequest,
    },
    wit::WitInterface,
};

const HTTP_HANDLER_P2_WASM: &[u8] = include_bytes!("../../tests/wasm/http_handler_p2.wasm");
const HTTP_HANDLER_P3_WASM: &[u8] = include_bytes!("../../tests/wasm/http_handler_p3.wasm");
const HTTP_SVC_PROXY_WASM: &[u8] = include_bytes!("../../tests/wasm/svc_http_proxy.wasm");

/// Body served by the `svc-http-proxy` fixture when a request carries no
/// `x-backend` header.
pub const DIRECT_BODY: &str = "hello from service";

/// Upper bound on any single bench request, warmup or measured. Generous
/// enough to never clip a real measurement; its purpose is turning a wedged
/// host into a loud failure instead of a silently hung bench run.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Copy, Clone, Debug)]
pub enum Flavor {
    P2,
    P3,
}

impl Flavor {
    pub fn name(self) -> &'static str {
        match self {
            Flavor::P2 => "p2",
            Flavor::P3 => "p3",
        }
    }

    pub fn wasm(self) -> &'static [u8] {
        match self {
            Flavor::P2 => HTTP_HANDLER_P2_WASM,
            Flavor::P3 => HTTP_HANDLER_P3_WASM,
        }
    }

    pub fn expected_body(self) -> &'static str {
        match self {
            Flavor::P2 => "hello from p2",
            Flavor::P3 => "hello from p3",
        }
    }

    /// `Host` header configured for (and sent to) this flavor's workload.
    pub fn host_header(self) -> &'static str {
        match self {
            Flavor::P2 => "bench-p2",
            Flavor::P3 => "bench-p3",
        }
    }
}

pub fn engine() -> Engine {
    Engine::builder().build().expect("failed to build engine")
}

pub fn http_host_interfaces(host: &str) -> Vec<WitInterface> {
    let mut config = HashMap::new();
    config.insert("host".to_string(), host.to_string());
    vec![WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: ["incoming-handler".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.2.2").expect("valid version")),
        config,
        name: None,
    }]
}

/// A started host bound to a concrete address, kept alive for the duration of
/// a benchmark group. Dropping a host does NOT abort a running service driver
/// (its spawned task holds the store and keeps ticking), so every consumer
/// must call [`BenchHost::shutdown`] — leaked instances accumulate across
/// iterations and eventually abort the process.
pub struct BenchHost {
    host: Arc<Host>,
    workload_id: String,
    pub addr: std::net::SocketAddr,
}

impl BenchHost {
    pub async fn shutdown(self) {
        let _ = self
            .host
            .workload_stop(WorkloadStopRequest {
                workload_id: self.workload_id,
            })
            .await;
        let _ = self.host.stop().await;
    }
}

pub async fn start_host_and_workload(
    req_for: impl FnOnce(&str) -> Workload,
) -> anyhow::Result<BenchHost> {
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();

    let host = HostBuilder::new()
        .with_engine(engine())
        .with_http_handler(Arc::new(http_server))
        .build()?;
    let host = host.start().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let resp = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: req_for("bench"),
        })
        .await?;
    anyhow::ensure!(
        resp.workload_status.workload_state == WorkloadState::Running,
        "workload did not start: {:?}: {}",
        resp.workload_status.workload_state,
        resp.workload_status.message
    );

    Ok(BenchHost {
        host,
        workload_id,
        addr,
    })
}

/// Start a host serving `flavor`'s HTTP component per-request. This backend
/// is not what we bench against; it instead hosts a component that serves an endpoint that the
/// client under bench calls.
pub async fn start_backend_host(flavor: Flavor) -> anyhow::Result<BenchHost> {
    start_host_and_workload(|host| Workload {
        namespace: "bench".to_string(),
        name: format!("backend-{}", flavor.name()),
        annotations: HashMap::new(),
        service: None,
        components: vec![Component {
            name: format!("hello-{}.wasm", flavor.name()),
            digest: None,
            bytes: bytes::Bytes::from_static(flavor.wasm()),
            local_resources: LocalResources::default(),
            // 0/0 → runtime defaults (128 reuses × 16 concurrent on the P3
            // instance-reuse path; ignored by the non-reuse path).
            pool_size: 0,
            max_invocations: 0,
        }],
        host_interfaces: http_host_interfaces(host),
        volumes: vec![],
    })
    .await
}

/// Start a host running the `svc-http-proxy` service workload. Loopback
/// egress is allowed so the routed benchmarks can reach a backend host.
pub async fn start_service_host() -> anyhow::Result<BenchHost> {
    start_host_and_workload(|host| Workload {
        namespace: "bench".to_string(),
        name: "bench-service".to_string(),
        annotations: HashMap::new(),
        service: Some(Service {
            digest: None,
            bytes: bytes::Bytes::from_static(HTTP_SVC_PROXY_WASM),
            local_resources: LocalResources {
                allowed_hosts: vec!["127.0.0.1".parse().expect("valid allowed host")].into(),
                ..LocalResources::default()
            },
            max_restarts: 0,
        }),
        components: vec![],
        host_interfaces: http_host_interfaces(host),
        volumes: vec![],
    })
    .await
}

pub fn bench_client() -> reqwest::Client {
    // One pooled HTTP/1.1 client for the client -> host hop so the outer
    // connection is not part of the measurement.
    reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .tcp_nodelay(true)
        .build()
        .expect("reqwest client")
}

/// GET the service; with `backend` set the service proxies to that authority.
/// Bounded by [`REQUEST_TIMEOUT`] so a wedged host fails the run instead of
/// hanging it.
pub async fn service_request(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    backend: Option<std::net::SocketAddr>,
) -> anyhow::Result<bytes::Bytes> {
    let mut req = client.get(format!("http://{addr}/"));
    if let Some(backend) = backend {
        req = req.header("x-backend", backend.to_string());
    }
    tokio::time::timeout(REQUEST_TIMEOUT, async {
        let resp = req.send().await?;
        anyhow::ensure!(resp.status().is_success(), "non-2xx: {}", resp.status());
        Ok(resp.bytes().await?)
    })
    .await
    .context("request timed out")?
}

/// Send one request and validate the body - used at setup so a misrouted or
/// silently-degraded path fails loudly instead of producing numbers for the
/// wrong thing.
pub async fn checked_request(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    backend: Option<std::net::SocketAddr>,
    expected_body: &str,
) -> anyhow::Result<()> {
    let body = service_request(client, addr, backend).await?;
    anyhow::ensure!(
        body == expected_body.as_bytes(),
        "unexpected body: {:?} (want {expected_body:?})",
        String::from_utf8_lossy(&body)
    );
    Ok(())
}
