//! HTTP invocation benchmarks for wash-runtime.
//!
//! Measures three dimensions for both WASIP2 and WASIP3 components:
//!
//! 1. **Cold invocation**  - end-to-end cost of building a host, starting a
//!    workload, and serving the first HTTP request. This captures component
//!    compilation, linker + `InstancePre` construction, and first-instance
//!    setup.
//! 2. **Hot invocation**  - steady-state single-request latency on a warm
//!    host (workload already resolved). This captures per-request cost:
//!    store/context allocation, instantiation, invocation, and response.
//! 3. **Throughput (RPS)**  - concurrent request throughput against the warm
//!    host. Uses N parallel clients to saturate the HTTP plane.
//!
//! The fixtures are intentionally minimal  - each returns a static body with
//! no plugin-backed host calls  - so that results isolate the runtime and are
//! directly comparable to `wasmtime serve` running the same component.
//!
//! Run with:
//! ```text
//! cargo bench -p wash-runtime --features wasip3 --bench http_invoke
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use common::Flavor;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::runtime::Runtime;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

fn flavor_host_header(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::P2 => "bench-p2",
        Flavor::P3 => "bench-p3",
    }
}

fn engine() -> Engine {
    // Enable P3 unconditionally so the same engine can serve both flavors.
    Engine::builder()
        .with_wasip3(true)
        .build()
        .expect("failed to build engine with wasip3")
}

fn http_host_interfaces(host: &str) -> Vec<WitInterface> {
    let mut config = HashMap::new();
    config.insert("host".to_string(), host.to_string());
    vec![WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: ["incoming-handler".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.2.2").unwrap()),
        config,
        name: None,
    }]
}

/// Holds a warm host bound to a concrete address with a workload resolved and
/// ready to serve requests. Kept alive for the duration of a benchmark group.
struct WarmHost {
    _host: Box<dyn std::any::Any + Send + Sync>,
    addr: std::net::SocketAddr,
    client: reqwest::Client,
    host_header: &'static str,
}

async fn start_warm_host(flavor: Flavor) -> anyhow::Result<WarmHost> {
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();

    let host = HostBuilder::new()
        .with_engine(engine())
        .with_http_handler(Arc::new(http_server))
        .build()?;

    let host = host.start().await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "bench".to_string(),
            name: format!("bench-{}", flavor.name()),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: format!("hello-{}.wasm", flavor.name()),
                digest: None,
                bytes: bytes::Bytes::from_static(flavor.wasm()),
                local_resources: LocalResources::default(),
                // 0/0 → the runtime picks sensible defaults. For the P3
                // instance-reuse path this means 128 reuses × 16 concurrent
                // (matches `wasmtime serve`). The non-reuse path ignores
                // both fields.
                pool_size: 0,
                max_invocations: 0,
            }],
            host_interfaces: http_host_interfaces(flavor_host_header(flavor)),
            volumes: vec![],
        },
    };
    host.workload_start(req).await?;

    // Reuse one HTTP/1.1 client with connection pooling so we are measuring
    // runtime work, not TCP/TLS handshakes.
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .tcp_nodelay(true)
        .build()?;

    // Correctness check  - also primes any one-time lazy caches before bench.
    let warmup = client
        .get(format!("http://{addr}/"))
        .header("HOST", flavor_host_header(flavor))
        .send()
        .await?;
    anyhow::ensure!(
        warmup.status().is_success(),
        "warmup request failed for {:?}: {}",
        flavor,
        warmup.status()
    );
    let body = warmup.text().await?;
    anyhow::ensure!(
        body == flavor.expected_body(),
        "unexpected warmup body for {:?}: {body:?}",
        flavor
    );

    Ok(WarmHost {
        _host: Box::new(host),
        addr,
        client,
        host_header: flavor_host_header(flavor),
    })
}

/// Cold invocation: builds host, starts workload, sends one request, drops.
/// Measures the full "first request" cost which is what matters for
/// scale-from-zero and short-lived workloads.
async fn cold_invocation(flavor: Flavor) -> anyhow::Result<()> {
    let warm = start_warm_host(flavor).await?;
    // start_warm_host already sends and validates one request.
    drop(warm);
    Ok(())
}

/// Hot invocation: one request on an already-warm host. Measures per-request
/// runtime cost (store + instance + invoke + response).
async fn hot_invocation(warm: &WarmHost) -> anyhow::Result<()> {
    let resp = warm
        .client
        .get(format!("http://{}/", warm.addr))
        .header("HOST", warm.host_header)
        .send()
        .await?;
    anyhow::ensure!(resp.status().is_success(), "non-2xx: {}", resp.status());
    // Consume body so the server-side stream completes before timing stops.
    let _ = resp.bytes().await?;
    Ok(())
}

fn bench_cold(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("cold_invocation");
    // Cold path is heavy (component compile + host build); keep sample count
    // low so runs are tolerable.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));

    for flavor in [Flavor::P2, Flavor::P3] {
        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.to_async(&rt)
                .iter(|| async move { cold_invocation(flavor).await.unwrap() });
        });
    }
    group.finish();
}

fn bench_hot_latency(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("hot_invocation");
    group.throughput(Throughput::Elements(1));
    group.measurement_time(Duration::from_secs(10));

    for flavor in [Flavor::P2, Flavor::P3] {
        let warm = rt.block_on(start_warm_host(flavor)).expect("warm host");
        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.to_async(&rt)
                .iter(|| async { hot_invocation(&warm).await.unwrap() });
        });
        drop(warm);
    }
    group.finish();
}

/// Throughput benchmark: measures RPS with N concurrent in-flight requests.
/// Each sample fires `BATCH` requests across `CONCURRENCY` workers and
/// criterion reports throughput in elements/sec = RPS.
fn bench_throughput(c: &mut Criterion) {
    const CONCURRENCY: usize = 32;
    const BATCH: usize = 256;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut group = c.benchmark_group("http_throughput");
    group.throughput(Throughput::Elements(BATCH as u64));
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(15));

    for flavor in [Flavor::P2, Flavor::P3] {
        let warm = rt.block_on(start_warm_host(flavor)).expect("warm host");
        let url = format!("http://{}/", warm.addr);
        let host_header = warm.host_header;
        let client = warm.client.clone();

        let failures = Arc::new(AtomicUsize::new(0));
        let failures_ref = failures.clone();
        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.to_async(&rt).iter_custom(|iters| {
                let url = url.clone();
                let client = client.clone();
                let failures = failures_ref.clone();
                async move {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let start = Instant::now();
                        let mut handles = Vec::with_capacity(CONCURRENCY);
                        let per_worker = BATCH / CONCURRENCY;
                        for _ in 0..CONCURRENCY {
                            let client = client.clone();
                            let url = url.clone();
                            let failures = failures.clone();
                            handles.push(tokio::spawn(async move {
                                for _ in 0..per_worker {
                                    match client.get(&url).header("HOST", host_header).send().await
                                    {
                                        Ok(resp) if resp.status().is_success() => {
                                            let _ = resp.bytes().await;
                                        }
                                        _ => {
                                            failures.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }));
                        }
                        for h in handles {
                            h.await.expect("worker");
                        }
                        total += start.elapsed();
                    }
                    total
                }
            });
        });
        let failed = failures.load(Ordering::Relaxed);
        if failed > 0 {
            eprintln!(
                "[http_throughput/{}] {failed} requests failed during bench run",
                flavor.name()
            );
        }
        drop(warm);
    }
    group.finish();
}

criterion_group!(benches, bench_cold, bench_hot_latency, bench_throughput);
criterion_main!(benches);
