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
//! cargo bench -p wash-runtime --bench http_invoke
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

use common::{Flavor, engine, http_host_interfaces};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::runtime::Runtime;

use wash_runtime::{
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, Ingress},
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

/// Holds a warm host bound to a concrete address with a workload resolved and
/// ready to serve requests. Kept alive for the duration of a benchmark group.
struct WarmHost {
    _host: Box<dyn std::any::Any + Send + Sync>,
    addr: std::net::SocketAddr,
    client: reqwest::Client,
    host_header: &'static str,
}

async fn start_warm_host(flavor: Flavor) -> anyhow::Result<WarmHost> {
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();

    let host = HostBuilder::new()
        .with_engine(engine())
        .with_http_handler(Arc::new(ingress))
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
                // No instance reuse: a store is built, instantiated and
                // dropped per request. That is the default, and it is the
                // baseline the pooled groups below are measured against.
                pool_size: 0,
                max_invocations: 0,
                max_concurrency: 1,
            }],
            host_interfaces: http_host_interfaces(flavor.host_header()),
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
        .header("HOST", flavor.host_header())
        .send()
        .await?;
    anyhow::ensure!(
        warmup.status().is_success(),
        "warmup request failed for {flavor:?}: {}",
        warmup.status()
    );
    let body = warmup.text().await?;
    anyhow::ensure!(
        body == flavor.expected_body(),
        "unexpected warmup body for {flavor:?}: {body:?}"
    );

    Ok(WarmHost {
        _host: Box::new(host),
        addr,
        client,
        host_header: flavor.host_header(),
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

/// An I/O-bound component: each request parks on the clock rather than
/// returning immediately. Instance reuse is only interesting for a guest that
/// *waits* — one that returns straight away never has two calls in flight, so
/// it cannot show what serving them concurrently is worth.
const HTTP_SLEEPER_WASM: &[u8] = include_bytes!("../tests/wasm/http_sleeper.wasm");

async fn start_sleeper_host(
    host_header: &'static str,
    pool_size: i32,
    max_concurrency: i32,
) -> anyhow::Result<WarmHost> {
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(engine())
        .with_http_handler(Arc::new(ingress))
        .build()?
        .start()
        .await?;

    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "bench".to_string(),
            name: host_header.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "sleeper".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_SLEEPER_WASM),
                local_resources: LocalResources::default(),
                pool_size,
                max_invocations: 0,
                max_concurrency,
            }],
            host_interfaces: http_host_interfaces(host_header),
            volumes: vec![],
        },
    })
    .await?;

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(256)
        .tcp_nodelay(true)
        .build()?;
    Ok(WarmHost {
        _host: Box::new(host),
        addr,
        client,
        host_header,
    })
}

/// Fire `n` requests at once and wait for all of them.
async fn sleeper_burst(warm: &WarmHost, n: usize) -> anyhow::Result<()> {
    let mut tasks = Vec::with_capacity(n);
    for _ in 0..n {
        let client = warm.client.clone();
        let url = format!("http://{}/", warm.addr);
        let host_header = warm.host_header;
        tasks.push(tokio::spawn(async move {
            let resp = client.get(url).header("HOST", host_header).send().await?;
            anyhow::ensure!(resp.status().is_success(), "non-2xx: {}", resp.status());
            let _ = resp.bytes().await?;
            Ok::<(), anyhow::Error>(())
        }));
    }
    for t in tasks {
        t.await??;
    }
    Ok(())
}

/// What instance reuse is worth to an I/O-bound component, at a concurrency no
/// single instance could serve without it.
///
/// * `ephemeral` — the default. Every request builds, instantiates and drops
///   its own store; the sleeps overlap, but the workload pays for a store per
///   request to get that.
/// * `warm_serial` — `pool_size: 4` with the default `max_concurrency: 1`.
///   Four calls run on warm instances and the rest still fall back to a store
///   each, because an instance serving one call at a time is busy for the whole
///   sleep.
/// * `warm_concurrent` — the same four instances at `max_concurrency: 16`,
///   which covers the whole burst. No store is built per request at all.
///
/// The fixture sleeps 100ms per request, so an iteration is roughly one sleep
/// plus whatever the policy spends on stores.
fn bench_pooled_throughput(c: &mut Criterion) {
    const CONCURRENCY: usize = 64;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("pooled throughput bench runtime");

    let mut group = c.benchmark_group("pooled_instance_throughput");
    group.throughput(Throughput::Elements(CONCURRENCY as u64));
    // Each iteration is a burst of sleeps; keep the run bounded.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));

    for (label, host_header, pool_size, max_concurrency) in [
        ("ephemeral", "bench-sleep-cold", 0, 1),
        ("warm_serial", "bench-sleep-warm", 4, 1),
        ("warm_concurrent", "bench-sleep-conc", 4, 16),
    ] {
        let warm = rt
            .block_on(start_sleeper_host(host_header, pool_size, max_concurrency))
            .expect("sleeper host");
        // Prime: the first burst pays for whatever instances the policy keeps.
        rt.block_on(sleeper_burst(&warm, CONCURRENCY))
            .expect("warmup burst");

        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.to_async(&rt)
                .iter(|| async { sleeper_burst(&warm, CONCURRENCY).await.expect("burst") });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_cold,
    bench_hot_latency,
    bench_throughput,
    bench_pooled_throughput
);
criterion_main!(benches);
