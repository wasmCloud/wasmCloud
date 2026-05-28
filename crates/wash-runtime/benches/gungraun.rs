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
//! Requires the `gungraun-runner` binary on `PATH` and `valgrind`
//! installed:
//! ```text
//! cargo install gungraun-runner --version 0.19.1
//! cargo bench -p wash-runtime --features wasip3 --bench gungraun
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{collections::HashMap, hint::black_box, sync::Arc};

use common::Flavor;
use gungraun::{library_benchmark, library_benchmark_group, main};
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
                host_interfaces: http_host_interfaces(flavor_host_header(flavor)),
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
            .header("HOST", flavor_host_header(flavor))
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
        host_header: flavor_host_header(flavor),
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

library_benchmark_group!(
    name = http;
    benchmarks = hot_invocation, cold_invocation
);

main!(library_benchmark_groups = http);
