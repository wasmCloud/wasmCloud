//! Valgrind/cachegrind-based instruction-count benchmarks for wash-runtime.
//!
//! Uses `gungraun` (formerly `iai-callgrind`, renamed at 0.17.0) to count
//! CPU instructions deterministically — which makes this suite suitable
//! for CI regression detection where wall-clock timing on shared runners
//! is too noisy. See `http_invoke` for the wall-clock counterpart.
//!
//! Measures the same hot-invocation path as `http_invoke`: a single HTTP
//! request through the full wash-runtime stack (hyper → router →
//! component). A cold-path measurement is included to track startup-cost
//! drift (component compile + host build + first request).
//!
//! The `service` group is the instruction-count counterpart of the
//! `service_http` wall-clock bench: a request served by a long-lived
//! `svc-http-proxy` service instance (direct), the same request routed
//! through the service's `wasi:http/client` import to a per-request HTTP
//! component on a second host (p2/p3), and the service's cold start.
//!
//! Requires the `gungraun-runner` binary on `PATH` and `valgrind`
//! installed:
//! ```text
//! cargo install gungraun-runner --version 0.19.1
//! cargo bench -p wash-runtime --bench gungraun
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{collections::HashMap, hint::black_box, sync::Arc};

use common::{
    BenchHost, DIRECT_BODY, Flavor, bench_client, checked_request, engine, http_host_interfaces,
    service_request, start_backend_host, start_service_host,
};
use gungraun::{library_benchmark, library_benchmark_group, main};
use tokio::runtime::Runtime;

use wash_runtime::{
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

/// Warm host bundled with the runtime that owns it. The host's drop impl
/// must run inside its tokio runtime, so the two have to die together.
struct Warm {
    rt: Runtime,
    _host: Box<dyn std::any::Any + Send + Sync>,
    addr: std::net::SocketAddr,
    client: reqwest::Client,
    host_header: &'static str,
}

fn setup_warm(flavor: Flavor) -> Warm {
    let rt = Runtime::new().expect("tokio runtime");
    let (host, addr, client) = rt.block_on(async {
        let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = http_server.addr();

        let host = HostBuilder::new()
            .with_engine(engine())
            .with_http_handler(Arc::new(http_server))
            .build()
            .unwrap();
        let host = host.start().await.unwrap();

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
                    pool_size: 0,
                    max_invocations: 0,
                }],
                host_interfaces: http_host_interfaces(flavor.host_header()),
                volumes: vec![],
            },
        };
        host.workload_start(req).await.unwrap();

        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(64)
            .tcp_nodelay(true)
            .build()
            .unwrap();

        // Warmup primes any one-time lazy state and validates the fixture.
        let warmup = client
            .get(format!("http://{addr}/"))
            .header("HOST", flavor.host_header())
            .send()
            .await
            .unwrap();
        assert!(
            warmup.status().is_success(),
            "warmup request failed for {flavor:?}: {}",
            warmup.status()
        );
        let body = warmup.text().await.unwrap();
        assert_eq!(body, flavor.expected_body(), "unexpected warmup body");

        (
            Box::new(host) as Box<dyn std::any::Any + Send + Sync>,
            addr,
            client,
        )
    });

    Warm {
        rt,
        _host: host,
        addr,
        client,
        host_header: flavor.host_header(),
    }
}

fn drop_warm(warm: Warm) {
    // Move drop out of the measurement window — the host shutdown path is
    // not what this bench is measuring.
    drop(warm);
}

// Hot path: one HTTP request against an already-warm host. Setup builds
// the host and is excluded from instruction counts; teardown drops it.
//
// fn name is the leaf segment of the callgrind output path and becomes
// the `param` in history.json rows (`hot_invocation.p2` etc.). The
// `gungraun_` prefix that used to live here was redundant with the
// (renamed) bench target name — keep this short and let the bench
// target carry the harness identity.
#[library_benchmark]
#[bench::p2(args = (Flavor::P2), setup = setup_warm, teardown = drop_warm)]
#[bench::p3(args = (Flavor::P3), setup = setup_warm, teardown = drop_warm)]
fn hot_invocation(warm: Warm) -> Warm {
    warm.rt.block_on(async {
        let resp = warm
            .client
            .get(format!("http://{}/", warm.addr))
            .header("HOST", warm.host_header)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "non-2xx: {}", resp.status());
        let _ = black_box(resp.bytes().await.unwrap());
    });
    warm
}

// Cold path: full host build + workload start + first request, all
// counted. Useful for tracking scale-from-zero drift. Teardown drops the
// host outside the measurement window — without this, the shutdown path
// (which we are *not* trying to measure) gets counted alongside startup.
#[library_benchmark]
#[bench::p2(args = (Flavor::P2), teardown = drop_warm)]
#[bench::p3(args = (Flavor::P3), teardown = drop_warm)]
fn cold_invocation(flavor: Flavor) -> Warm {
    setup_warm(flavor)
}

/// Warm service (plus optional routed backend) bundled with the runtime that
/// owns them. Unlike the component hosts above, dropping is not enough at
/// teardown: the service driver task keeps its store alive, so workload and
/// host must be stopped explicitly — see [`shutdown_service`].
struct WarmService {
    rt: Runtime,
    service: BenchHost,
    backend: Option<BenchHost>,
    client: reqwest::Client,
}

/// Start a service host (plus a backend host for that flavor's HTTP
/// component when `backend` is set), validated with exactly one round-trip —
/// the shape the cold benchmark measures.
fn start_service(backend: Option<Flavor>) -> WarmService {
    let rt = Runtime::new().expect("tokio runtime");
    let (service, backend, client) = rt.block_on(async {
        let service = start_service_host().await.unwrap();
        let (backend, expected_body) = match backend {
            Some(flavor) => (
                Some(start_backend_host(flavor).await.unwrap()),
                flavor.expected_body(),
            ),
            None => (None, DIRECT_BODY),
        };
        let client = bench_client();
        checked_request(
            &client,
            service.addr,
            backend.as_ref().map(|b| b.addr),
            expected_body,
        )
        .await
        .unwrap();
        (service, backend, client)
    });
    WarmService {
        rt,
        service,
        backend,
        client,
    }
}

/// [`start_service`] plus extra validated warmup round-trips, so a measured
/// hot call starts from a steady state (on the routed path the extras also
/// settle the backend's reused p3 instance) rather than right behind
/// first-request setup. Setup for the hot benchmarks only — the cold
/// benchmark measures [`start_service`] with its single round-trip.
fn setup_service(backend: Option<Flavor>) -> WarmService {
    let warm = start_service(backend);
    warm.rt.block_on(async {
        let backend_addr = warm.backend.as_ref().map(|b| b.addr);
        for _ in 0..2 {
            let _ = service_request(&warm.client, warm.service.addr, backend_addr)
                .await
                .unwrap();
        }
    });
    warm
}

/// Teardown outside the measurement window: stop the workload(s) and host(s).
fn shutdown_service(warm: WarmService) {
    let WarmService {
        rt,
        service,
        backend,
        client,
    } = warm;
    drop(client);
    rt.block_on(async {
        service.shutdown().await;
        if let Some(backend) = backend {
            backend.shutdown().await;
        }
    });
}

// Hot service path: one request answered directly by the long-lived service
// instance (HTTP server -> ingress channel -> co-driven `http/handler`).
#[library_benchmark]
#[bench::direct(args = (None), setup = setup_service, teardown = shutdown_service)]
fn hot_service(warm: WarmService) -> WarmService {
    warm.rt.block_on(async {
        let body = service_request(&warm.client, warm.service.addr, None)
            .await
            .unwrap();
        let _ = black_box(body);
    });
    warm
}

// Routed service path: the request is forwarded through the service's
// `wasi:http/client` import to a per-request HTTP component on a second host.
#[library_benchmark]
#[bench::p2(args = (Some(Flavor::P2)), setup = setup_service, teardown = shutdown_service)]
#[bench::p3(args = (Some(Flavor::P3)), setup = setup_service, teardown = shutdown_service)]
fn service_to_component(warm: WarmService) -> WarmService {
    let backend_addr = warm.backend.as_ref().map(|b| b.addr);
    warm.rt.block_on(async {
        let body = service_request(&warm.client, warm.service.addr, backend_addr)
            .await
            .unwrap();
        let _ = black_box(body);
    });
    warm
}

// Cold service path: host build + service workload start (trigger driver,
// ingress registration) + first request, all counted. Teardown stops the
// workload and host outside the measurement window.
#[library_benchmark]
#[bench::direct(args = (None), teardown = shutdown_service)]
fn cold_service(backend: Option<Flavor>) -> WarmService {
    start_service(backend)
}

library_benchmark_group!(
    name = http;
    benchmarks = hot_invocation, cold_invocation
);

library_benchmark_group!(
    name = service;
    benchmarks = hot_service, service_to_component, cold_service
);

main!(library_benchmark_groups = http, service);
