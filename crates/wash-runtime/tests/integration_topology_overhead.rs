//! Gated measurement of what each workload *topology* costs, independent of
//! what the workload does.
//!
//! The shared-Postgres-pool template is a service that routes to sub-components
//! over linked calls. That shape buys isolation and a state/compute split, and
//! this measures what it costs against the two simpler shapes, with the work
//! held to a trivial constant in every case so the difference is the topology:
//!
//!  * **component** — one component exporting `wasi:http/handler`, no service.
//!    A store is built and instantiated per request unless `pool_size` says
//!    otherwise; measured both ways.
//!  * **service** — one service exporting `wasi:http/handler`. Instantiated
//!    once; every request is a concurrent task on that *one* instance, which
//!    is also its ceiling.
//!  * **service + component** — a service that calls a linked component per
//!    request (the template's shape), measured with the callee cold (a store
//!    per call, the default) and warm (`pool_size`).
//!
//! The response bodies are not byte-identical across fixtures (13, ~18 and 2
//! bytes), but all are far too small for that to matter next to per-request
//! store and instantiation costs.
//!
//! Run with:
//!   cargo test --test integration_topology_overhead -- --ignored --nocapture

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use tokio::time::Instant;

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

/// One component exporting `wasi:http/handler@0.3`, nothing else.
const HTTP_HANDLER_P3_WASM: &[u8] = include_bytes!("wasm/http_handler_p3.wasm");
/// A service exporting `wasi:http/handler@0.3` and no `cli/run`.
const SVC_NO_RUN_WASM: &[u8] = include_bytes!("wasm/svc_no_run.wasm");
/// An HTTP handler that calls a plain-value async export of a linked component.
const EPHEMERAL_CALLER_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_caller_p3.wasm");
const EPHEMERAL_CALLEE_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_callee_p3.wasm");

const LEVELS: &[usize] = &[1, 2, 4, 8, 16, 32];
const PHASE: Duration = Duration::from_secs(5);
const WARMUP: usize = 50;

fn resources() -> LocalResources {
    LocalResources::default()
}

/// A workload with a single component serving HTTP — no service at all.
/// `warm` is its `pool_size`.
fn component_only(host: &str, warm: i32) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "bench".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "handler".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_HANDLER_P3_WASM),
                local_resources: resources(),
                pool_size: warm,
                max_invocations: 0,
            }],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

/// A workload with a single service serving HTTP — no sub-components.
fn service_only(host: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "bench".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(SVC_NO_RUN_WASM),
                local_resources: resources(),
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

/// The template's shape: a service that calls a linked sub-component per
/// request. `warm` is the callee's `pool_size`.
fn service_plus_component(host: &str, warm: i32) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "bench".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(EPHEMERAL_CALLER_P3_WASM),
                local_resources: resources(),
                max_restarts: 0,
            }),
            components: vec![Component {
                name: "callee".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(EPHEMERAL_CALLEE_P3_WASM),
                local_resources: resources(),
                pool_size: warm,
                max_invocations: 0,
            }],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

struct Stats {
    elapsed: Duration,
    errors: u64,
    /// Sorted per-request latencies, in microseconds.
    latencies: Vec<u64>,
}

impl Stats {
    fn rps(&self) -> f64 {
        self.latencies.len() as f64 / self.elapsed.as_secs_f64()
    }
    fn pct(&self, p: f64) -> f64 {
        if self.latencies.is_empty() {
            return f64::NAN;
        }
        let idx = ((self.latencies.len() as f64 - 1.0) * p).round() as usize;
        self.latencies
            .get(idx)
            .map(|us| *us as f64 / 1000.0)
            .unwrap_or(f64::NAN)
    }
}

async fn load(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host: &'static str,
    concurrency: usize,
) -> Stats {
    for _ in 0..WARMUP {
        let _ = client
            .get(format!("http://{addr}/"))
            .header("HOST", host)
            .send()
            .await;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let errors = Arc::new(AtomicU64::new(0));
    let mut workers = Vec::new();
    let started = Instant::now();
    for _ in 0..concurrency {
        let client = client.clone();
        let stop = Arc::clone(&stop);
        let errors = Arc::clone(&errors);
        workers.push(tokio::spawn(async move {
            let mut latencies = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let t0 = Instant::now();
                let ok = match client
                    .get(format!("http://{addr}/"))
                    .header("HOST", host)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let status = resp.status();
                        let drained = resp.bytes().await.is_ok();
                        status.is_success() && drained
                    }
                    Err(_) => false,
                };
                if ok {
                    latencies.push(t0.elapsed().as_micros() as u64);
                } else {
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            }
            latencies
        }));
    }
    tokio::time::sleep(PHASE).await;
    stop.store(true, Ordering::Relaxed);

    let mut latencies = Vec::new();
    for w in workers {
        latencies.extend(w.await.unwrap_or_default());
    }
    let elapsed = started.elapsed();
    latencies.sort_unstable();
    Stats {
        elapsed,
        errors: errors.load(Ordering::Relaxed),
        latencies,
    }
}

/// Sweep one topology on its own host, returning req/s per concurrency level.
async fn sweep(label: &str, host: &'static str, request: WorkloadStartRequest) -> Result<Vec<f64>> {
    let (addr, host_api) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    host_api.workload_start(request).await?;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .timeout(Duration::from_secs(30))
        .build()?;

    println!("\n=== {label} ===");
    println!(
        "{:>5}  {:>10}  {:>9}  {:>9}  {:>7}",
        "conc", "req/s", "p50 ms", "p99 ms", "errors"
    );
    let mut rps = Vec::new();
    for &c in LEVELS {
        let s = load(&client, addr, host, c).await;
        println!(
            "{:>5}  {:>10.1}  {:>9.3}  {:>9.3}  {:>7}",
            c,
            s.rps(),
            s.pct(0.50),
            s.pct(0.99),
            s.errors
        );
        rps.push(s.rps());
    }
    drop(host_api);
    Ok(rps)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement; run with --ignored"]
async fn bench_topology_overhead() -> Result<()> {
    let component = sweep(
        "component only, cold (store + instantiate per request)",
        "topo-c",
        component_only("topo-c", 0),
    )
    .await?;
    let component_warm = sweep(
        "component only, warm (pool_size 4)",
        "topo-cw",
        component_only("topo-cw", 4),
    )
    .await?;
    let service = sweep(
        "service only (one long-lived instance)",
        "topo-s",
        service_only("topo-s"),
    )
    .await?;
    let cold = sweep(
        "service + linked component, callee cold (store per call)",
        "topo-sc",
        service_plus_component("topo-sc", 0),
    )
    .await?;
    let warm = sweep(
        "service + linked component, callee warm (pool_size 4)",
        "topo-sw",
        service_plus_component("topo-sw", 4),
    )
    .await?;

    println!("\n=== relative throughput (higher is better; service-only = 1.00) ===");
    println!(
        "{:>5}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}",
        "conc", "comp", "comp-warm", "service", "svc+comp", "svc+warm"
    );
    for (i, &c) in LEVELS.iter().enumerate() {
        let base = service.get(i).copied().unwrap_or(f64::NAN);
        let rel = |v: Option<&f64>| v.copied().unwrap_or(f64::NAN) / base;
        println!(
            "{:>5}  {:>10.2}  {:>10.2}  {:>10.2}  {:>10.2}  {:>10.2}",
            c,
            rel(component.get(i)),
            rel(component_warm.get(i)),
            1.0,
            rel(cold.get(i)),
            rel(warm.get(i)),
        );
    }
    Ok(())
}
