//! The host-wide guest memory budget, driven by a p3 guest that really grows.
//!
//! The `http-memory-grow` fixture grows its own linear memory a page at a time
//! through `memory.grow` and reports how much it got and whether it was
//! refused. That is what makes the two modes distinguishable end to end: in
//! `Count` the guest always gets everything it asked for, in `Enforce` it stops
//! at the budget and *says so* instead of trapping.
//!
//! What the unit tests in `engine::guest_memory` cannot cover is whether the
//! limiter is installed on the stores the host actually builds. These tests are
//! that check: they never touch the budget directly, only the guest's own
//! report of what the host let it have.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::{
    engine::{
        Engine,
        guest_memory::{GuestMemoryBudget, GuestMemoryMode},
        host_memory::HostMemoryBudgets,
    },
    host::{
        HostApi, HostBuilder,
        http::{DynamicRouter, Ingress},
    },
    plugin::{wasi_config::DynamicConfig, wasi_logging::TracingLogger},
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadStopRequest},
    wit::WitInterface,
};

const HTTP_MEMORY_GROW_WASM: &[u8] = include_bytes!("wasm/http_memory_grow.wasm");

const MIB: u64 = 1024 * 1024;

/// The budget these tests run under. Small enough that one fixture request can
/// cross it, and well under `default_heap_memory` so it is the *aggregate*
/// ceiling under test rather than wasmtime's per-memory one.
const BUDGET_MIB: u64 = 48;

/// What the guest asks for in a single request. Two of these cross
/// [`BUDGET_MIB`]; one does not.
const REQUEST_MIB: u64 = 32;

/// What the guest reported about one `/grow` request.
#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
struct GrowReport {
    granted_mib: u64,
    refused: bool,
}

async fn start_host(
    mode: GuestMemoryMode,
) -> Result<(std::net::SocketAddr, impl HostApi, Arc<GuestMemoryBudget>)> {
    let engine = Engine::builder()
        .with_host_memory(HostMemoryBudgets::resolve(Some(BUDGET_MIB * MIB), None, None).unwrap())
        .with_guest_memory_mode(mode)
        .build()?;
    let budget = Arc::clone(engine.guest_memory());
    let ingress = Ingress::new(DynamicRouter::default(), "127.0.0.1:0".parse()?).await?;
    let bound_addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;
    Ok((bound_addr, host, budget))
}

/// A workload serving `http-memory-grow` under `name` as its HTTP host header.
///
/// `pool_size: 1` keeps one warm store per workload, so a workload's grown
/// pages stay resident between requests — which is what lets these tests
/// accumulate toward the budget instead of having every request start over.
fn grow_workload(name: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: name.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-memory-grow.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_MEMORY_GROW_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 128,
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
            }],
            host_interfaces: vec![WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.2").unwrap()),
                config: HashMap::from([("host".to_string(), name.to_string())]),
                name: None,
            }],
            volumes: vec![],
        },
    }
}

async fn grow(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
    mib: u64,
) -> Result<GrowReport> {
    let response = timeout(
        Duration::from_secs(30),
        client
            .get(format!("http://{addr}/grow?mib={mib}"))
            .header("HOST", host_header)
            .send(),
    )
    .await
    .context("grow request timed out")?
    .context("grow request failed")?;
    assert_eq!(
        response.status().as_u16(),
        200,
        "a refusal must reach the guest as -1, never as a trap"
    );

    let body = response.text().await.context("failed to read grow body")?;
    serde_json::from_str(&body).with_context(|| format!("unparseable grow reply: {body}"))
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

/// The upgrade-safety guarantee, end to end: a host that gains a budget it
/// never asked for must serve exactly what it served before. Every guest gets
/// everything it asks for, past the budget included — and the host still
/// counts it, which is the number no host has today.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn count_mode_allows_growth_past_the_budget_and_still_accounts_for_it() -> Result<()> {
    init_tracing();
    let (addr, host, budget) = start_host(GuestMemoryMode::Count).await?;
    let client = reqwest::Client::new();

    for name in ["grow-a", "grow-b"] {
        host.workload_start(grow_workload(name))
            .await
            .with_context(|| format!("failed to start {name}"))?;
    }

    for name in ["grow-a", "grow-b"] {
        let report = grow(&client, addr, name, REQUEST_MIB).await?;
        assert_eq!(
            report,
            GrowReport {
                granted_mib: REQUEST_MIB,
                refused: false
            },
            "{name} must get everything it asked for in count mode"
        );
    }

    // Together the two are past a 48MiB budget, which is the whole point: the
    // host has to *notice* aggregate creep it is not stopping.
    assert!(
        budget.high_water() >= 2 * REQUEST_MIB * MIB,
        "the high-water mark must cover both guests: {}",
        budget.high_water()
    );
    assert!(
        budget.would_refuse() > 0,
        "count mode must record what enforcement would have refused"
    );
    assert_eq!(budget.refused(), 0, "count mode refuses nothing");

    Ok(())
}

/// Aggregate creep, caught: two workloads each well under `default_heap_memory`
/// and each individually affordable, together past the host's budget. The
/// second is cut off partway through — and it finds out by `memory.grow`
/// returning -1, not by trapping.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enforce_mode_refuses_the_growth_that_crosses_the_budget() -> Result<()> {
    init_tracing();
    let (addr, host, budget) = start_host(GuestMemoryMode::Enforce).await?;
    let client = reqwest::Client::new();

    for name in ["grow-a", "grow-b"] {
        host.workload_start(grow_workload(name))
            .await
            .with_context(|| format!("failed to start {name}"))?;
    }

    let first = grow(&client, addr, "grow-a", REQUEST_MIB).await?;
    assert_eq!(
        first,
        GrowReport {
            granted_mib: REQUEST_MIB,
            refused: false
        },
        "the first guest is comfortably inside the budget"
    );

    let second = grow(&client, addr, "grow-b", REQUEST_MIB).await?;
    assert!(
        second.refused,
        "the second guest must be cut off at the budget, got {second:?}"
    );
    assert!(
        second.granted_mib < REQUEST_MIB,
        "it must be cut off partway, not merely fail: {second:?}"
    );

    assert!(budget.refused() > 0, "the budget must record the refusal");
    assert_eq!(
        budget.would_refuse(),
        0,
        "enforce mode refuses rather than counting"
    );
    assert!(
        budget.in_use() <= budget.cap(),
        "the budget must never admit past its cap: {} > {}",
        budget.in_use(),
        budget.cap()
    );

    Ok(())
}

/// The leak the release-on-drop path exists to prevent. A workload that has
/// filled the budget and is then stopped must give every byte back — otherwise
/// the budget ratchets down until the host refuses everything, which is a worse
/// failure than the one it was built to prevent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_workload_returns_its_memory_to_the_budget() -> Result<()> {
    init_tracing();
    let (addr, host, budget) = start_host(GuestMemoryMode::Enforce).await?;
    let client = reqwest::Client::new();

    let hog = grow_workload("grow-hog");
    let hog_id = hog.workload_id.clone();
    host.workload_start(hog)
        .await
        .context("failed to start hog")?;
    host.workload_start(grow_workload("grow-after"))
        .await
        .context("failed to start the second workload")?;

    let filled = grow(&client, addr, "grow-hog", REQUEST_MIB).await?;
    assert_eq!(filled.granted_mib, REQUEST_MIB);
    // Captured rather than compared against a threshold: the hog holds its
    // grown pages *plus* an instantiation baseline, so "below 32MiB" would be
    // satisfied by returning 3MiB and leaking the rest.
    let held = budget.in_use();
    assert!(held >= REQUEST_MIB * MIB);

    host.workload_stop(WorkloadStopRequest {
        workload_id: hog_id,
    })
    .await
    .context("failed to stop the hog")?;

    // The stop tears the workload's stores down asynchronously; the bytes come
    // back as each one drops.
    let released = |budget: &GuestMemoryBudget| held.saturating_sub(budget.in_use());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while released(&budget) < REQUEST_MIB * MIB && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        released(&budget) >= REQUEST_MIB * MIB,
        "a stopped workload must return everything it grew: held {held}, now {}, released {}",
        budget.in_use(),
        released(&budget),
    );

    // And the returned bytes are genuinely spendable again, not merely
    // uncounted: the next guest gets the full ask.
    let after = grow(&client, addr, "grow-after", REQUEST_MIB).await?;
    assert_eq!(
        after,
        GrowReport {
            granted_mib: REQUEST_MIB,
            refused: false
        },
        "the freed budget must be usable by the next workload"
    );

    Ok(())
}
