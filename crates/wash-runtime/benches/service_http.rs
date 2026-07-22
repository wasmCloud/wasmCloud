//! Service HTTP benchmarks for wash-runtime.
//!
//! A wasmCloud *service* is a long-lived p3 instance: the host co-drives
//! `wasi:cli/run` with `wasi:http/handler` on one store, and the HTTP server
//! delivers inbound requests to the live instance over a channel instead of
//! instantiating a component per request. These benchmarks measure that plane
//! with the `svc-http-proxy` fixture:
//!
//! 1. **Cold start** - end-to-end cost of building a host, starting a service
//!    workload (trigger-service driver, ingress registration), and serving the
//!    first HTTP request. Teardown runs every iteration but outside the timed
//!    window, so the numbers are comparable to `http_invoke`'s cold path.
//! 2. **Hot invocation (direct)** - steady-state single-request latency
//!    against the warm service instance answering from its own handler.
//! 3. **Throughput (direct)** - concurrent RPS against the warm service
//!    instance, saturating the service ingress channel.
//! 4. **Service -> component (routed)** - the service forwards each request to
//!    an HTTP component workload on a second host via its imported
//!    `wasi:http/client`. This exercises the full routing chain: service
//!    ingress -> guest handler -> outbound egress (allowed-hosts policy,
//!    client span, stock p3 send) -> TCP -> second host's HTTP server ->
//!    router -> per-request component instance -> response streamed back
//!    through the service. Benched against both P2 and P3 backends.
//!
//! The routed leg opens a fresh TCP connection per request (the stock p3
//! egress has no connection pooling), and each connection's client-side
//! TIME_WAIT holds a loopback ephemeral port for 30-60s. The routed loop is
//! therefore burst-paced below the port pool's drain rate (~500
//! connections/s on both macOS and Linux) - see
//! [`bench_service_to_component`] for why bursts rather than a per-request
//! gap.
//!
//! Run with:
//! ```text
//! cargo bench -p wash-runtime --bench service_http
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::runtime::Runtime;

use common::{
    DIRECT_BODY, Flavor, REQUEST_TIMEOUT, bench_client, checked_request, service_request,
    start_backend_host, start_service_host,
};

/// Cold start: build the host, start the service workload, serve + validate
/// the first request. The scale-from-zero cost of a service. Shutdown runs
/// per iteration but outside the timed window - it keeps hundreds of
/// iterations from accumulating live service instances without folding the
/// stop path into the startup numbers.
fn bench_cold(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("service_cold_invocation");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("p3-service", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let service = start_service_host().await.expect("service host");
                let client = bench_client();
                checked_request(&client, service.addr, None, DIRECT_BODY)
                    .await
                    .expect("first request");
                total += start.elapsed();
                service.shutdown().await;
            }
            total
        });
    });
    group.finish();
}

/// Hot latency of the direct path: client -> HTTP server -> service ingress
/// channel -> live instance handler -> streamed response.
fn bench_hot_direct(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("service_hot_invocation");
    group.throughput(Throughput::Elements(1));
    group.measurement_time(Duration::from_secs(10));

    let (service, client) = rt.block_on(async {
        let service = start_service_host().await.expect("service host");
        let client = bench_client();
        checked_request(&client, service.addr, None, DIRECT_BODY)
            .await
            .expect("warmup");
        (service, client)
    });

    group.bench_function("direct", |b| {
        b.to_async(&rt).iter(|| async {
            service_request(&client, service.addr, None).await.unwrap();
        });
    });
    group.finish();
    rt.block_on(service.shutdown());
}

/// Concurrent throughput of the direct path: RPS with N in-flight requests
/// against the single live service instance.
fn bench_throughput_direct(c: &mut Criterion) {
    const CONCURRENCY: usize = 32;
    const BATCH: usize = 256;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut group = c.benchmark_group("service_http_throughput");
    group.throughput(Throughput::Elements(BATCH as u64));
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(15));

    let (service, client) = rt.block_on(async {
        let service = start_service_host().await.expect("service host");
        let client = bench_client();
        checked_request(&client, service.addr, None, DIRECT_BODY)
            .await
            .expect("warmup");
        (service, client)
    });
    let url = format!("http://{}/", service.addr);

    let failures = Arc::new(AtomicUsize::new(0));
    let failures_ref = failures.clone();
    group.bench_function("direct", |b| {
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
                                let ok = tokio::time::timeout(REQUEST_TIMEOUT, async {
                                    match client.get(&url).send().await {
                                        Ok(resp) if resp.status().is_success() => {
                                            resp.bytes().await.is_ok()
                                        }
                                        _ => false,
                                    }
                                })
                                .await
                                .unwrap_or(false);
                                if !ok {
                                    failures.fetch_add(1, Ordering::Relaxed);
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
        eprintln!("[service_http_throughput/direct] {failed} requests failed during bench run");
    }
    group.finish();
    rt.block_on(service.shutdown());
}

/// Hot latency of the routed path: the service forwards each request to a
/// per-request HTTP component on a second host and streams the backend's
/// response through.
///
/// Every routed request opens a fresh outbound TCP connection (stock p3
/// egress has no pooling) whose client-side TIME_WAIT parks an ephemeral
/// port for 30s (macOS, ~16k-port pool) or 60s (Linux, ~28k). Left unpaced,
/// the loop empties the pool mid-run and measures the OS refusing
/// connections. Pacing every request instead distorts the numbers the other
/// way: an idle gap before each request lets the OS power-manage the
/// process (timer oversleep, frequency ramp, efficiency-core placement) and
/// each request then pays a multi-millisecond wake-up tax.
///
/// So the loop paces in BURSTS: runs of back-to-back requests (timed, warm)
/// separated by one long untimed pause that keeps the average connection
/// rate under the pool's drain rate (~500/s on both platforms), with a
/// couple of untimed warmer requests after each pause so the timed burst
/// never includes the post-idle cold edge.
fn bench_service_to_component(c: &mut Criterion) {
    /// Timed requests per pacing cycle.
    const BURST: u64 = 64;
    /// Untimed pause between bursts. (BURST + warmers) / PAUSE stays well
    /// under the ephemeral-port drain rate.
    const PAUSE: Duration = Duration::from_millis(200);
    /// Untimed requests absorbing the post-pause wake-up cost.
    const WARMERS: usize = 2;

    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("service_to_component");
    group.throughput(Throughput::Elements(1));
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(1));

    for flavor in [Flavor::P2, Flavor::P3] {
        let (service, backend, client) = rt.block_on(async {
            let service = start_service_host().await.expect("service host");
            let backend = start_backend_host(flavor).await.expect("backend host");
            let client = bench_client();
            checked_request(
                &client,
                service.addr,
                Some(backend.addr),
                flavor.expected_body(),
            )
            .await
            .expect("routed warmup");
            (service, backend, client)
        });
        let (service_addr, backend_addr) = (service.addr, backend.addr);

        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.to_async(&rt).iter_custom(|iters| {
                let client = client.clone();
                async move {
                    let mut total = Duration::ZERO;
                    let mut in_burst = 0u64;
                    for _ in 0..iters {
                        if in_burst == BURST {
                            in_burst = 0;
                            tokio::time::sleep(PAUSE).await;
                            for _ in 0..WARMERS {
                                let _ = service_request(&client, service_addr, Some(backend_addr))
                                    .await;
                            }
                        }
                        let start = Instant::now();
                        service_request(&client, service_addr, Some(backend_addr))
                            .await
                            .unwrap();
                        total += start.elapsed();
                        in_burst += 1;
                    }
                    total
                }
            });
        });
        rt.block_on(async {
            service.shutdown().await;
            backend.shutdown().await;
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_cold,
    bench_hot_direct,
    bench_throughput_direct,
    bench_service_to_component
);
criterion_main!(benches);
