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
        http::{DevRouter, Ingress},
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
const HTTP_SERVER_P3_WASM: &[u8] = include_bytes!("../../tests/wasm/http_server_p3.wasm");

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

/// Per-request latency summary for `iter_custom` benches.
///
/// Accumulates individual request latencies so a run can report RPS and
/// p50/p90/p99 in addition to Criterion's aggregate batch timing.
#[derive(Debug, Default)]
pub struct BenchStats {
    /// Sum of timed per-request durations, excluding untimed pacing pauses
    /// and warmer requests.
    pub total_timed: Duration,

    /// Total number of failed requests (dropped, timeout, wrong body, ...).
    pub errors: u64,
    /// Every successful individual request's round-trip duration in milliseconds.
    pub latencies: Vec<f64>,
}

impl BenchStats {
    /// Merge one `iter_custom` batch into this accumulator.
    pub fn extend(&mut self, latencies: Vec<f64>, total_timed: Duration, errors: u64) {
        self.latencies.extend(latencies);
        self.total_timed += total_timed;
        self.errors += errors;
    }

    pub fn sort_latencies(&mut self) {
        self.latencies.sort_by(|a, b| a.total_cmp(b));
    }

    fn is_sorted(&self) -> bool {
        self.latencies.windows(2).all(|w| match w {
            [a, b] => a.total_cmp(b) != std::cmp::Ordering::Greater,
            _ => true,
        })
    }

    /// Nearest-rank percentile in `[0.0, 1.0]`. Requires sorted latencies;
    /// call [`BenchStats::sort_latencies`] (or `print_summary`, which sorts
    /// internally) first.
    pub fn pct(&self, p: f64) -> f64 {
        assert!(
            self.is_sorted(),
            "BenchStats::pct requires sorted latencies"
        );
        if self.latencies.is_empty() {
            return f64::NAN;
        }
        let p = p.clamp(0.0, 1.0);
        let index = ((self.latencies.len() as f64 - 1.0) * p).round() as usize;
        match self.latencies.get(index) {
            Some(&v) => v,
            None => f64::NAN,
        }
    }

    /// Mean timed-request rate: `(successes + errors) / total_timed`,
    /// excluding pacing pauses from the denominator.
    pub fn rps(&self) -> f64 {
        let secs = self.total_timed.as_secs_f64();
        if secs > 0.0 {
            (self.latencies.len() as f64 + self.errors as f64) / secs
        } else {
            0.0
        }
    }

    pub fn print_summary(&mut self, name: &str) {
        self.sort_latencies();
        println!(
            "[{name}] RPS: {:>8.1} | p50: {:>6.2} ms | p90: {:>6.2} ms | p99: {:>6.2} ms | errors: {}",
            self.rps(),
            self.pct(0.50),
            self.pct(0.90),
            self.pct(0.99),
            self.errors
        );
    }
}

pub async fn start_host_and_workload(
    req_for: impl FnOnce(&str) -> Workload,
) -> anyhow::Result<BenchHost> {
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();

    let host = HostBuilder::new()
        .with_engine(engine())
        .with_http_handler(Arc::new(ingress))
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
            max_concurrency: 1,
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
            ports: Vec::new(),
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
    endpoint: Option<&str>,
) -> anyhow::Result<bytes::Bytes> {
    service_request_path(client, addr, backend, endpoint).await
}

pub async fn service_request_path(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    backend: Option<std::net::SocketAddr>,
    endpoint: Option<&str>,
) -> anyhow::Result<bytes::Bytes> {
    let endpoint = endpoint.unwrap_or("/");
    let path = if endpoint.starts_with('/') {
        endpoint.to_string()
    } else {
        format!("/{endpoint}")
    };
    let mut req = client.get(format!("http://{addr}{path}"));
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
    expected_body: impl AsRef<[u8]>,
    endpoint: Option<&str>,
) -> anyhow::Result<()> {
    let body = service_request_path(client, addr, backend, endpoint).await?;
    let expected = expected_body.as_ref();
    anyhow::ensure!(
        body == expected,
        "unexpected body: got {} bytes, want {} bytes",
        body.len(),
        expected.len()
    );
    Ok(())
}

/// Start a backend host running `http_server_p3.wasm` on a published port.
///
/// The service binds `127.0.0.1:8080` in virtual loopback, and the host splices
/// external TCP connections arriving at the published host port into it.
pub async fn start_spliced_backend_host() -> anyhow::Result<BenchHost> {
    use wash_runtime::host::declared_port::{DeclaredPort, Protocol};
    use wash_runtime::host::ports::{PortTable, PublishConfig, PublishContext};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let host_port = listener.local_addr()?.port();
    drop(listener);

    let table = PortTable::new();
    let config = PublishConfig {
        enabled: true,
        readiness_timeout: Duration::from_secs(10),
        ..Default::default()
    };

    let host = HostBuilder::new()
        .with_engine(engine())
        .with_publish_context(PublishContext::new(table, config))
        .build()?;
    let host = host.start().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let resp = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: Workload {
                namespace: "bench".to_string(),
                name: "backend-spliced".to_string(),
                annotations: HashMap::new(),
                service: Some(Service {
                    digest: None,
                    bytes: bytes::Bytes::from_static(HTTP_SERVER_P3_WASM),
                    local_resources: LocalResources::default(),
                    max_restarts: 0,
                    ports: vec![DeclaredPort {
                        name: "http".into(),
                        port: 8080,
                        protocol: Protocol::Tcp,
                        publish: Some(host_port),
                        bind: None,
                    }],
                }),
                components: vec![],
                host_interfaces: vec![],
                volumes: vec![],
            },
        })
        .await?;
    anyhow::ensure!(
        resp.workload_status.workload_state == WorkloadState::Running,
        "workload did not start: {:?}: {}",
        resp.workload_status.workload_state,
        resp.workload_status.message
    );

    let addr: std::net::SocketAddr = format!("127.0.0.1:{host_port}").parse()?;
    Ok(BenchHost {
        host,
        workload_id,
        addr,
    })
}
