//! How many HTTP workloads one host holds before `max-process-memory` stops
//! it taking more, warm against scale-to-zero.
//!
//! The two hit different ceilings, so the bench runs both and compares them:
//!
//! - **warm** (`pool_size: 1`): every workload keeps an instance parked and
//!   answers its first request before the next step starts. Each one holds
//!   pooled slots and a live linear memory, so the pool's totals and the
//!   memory limit compete to stop it.
//! - **scale-to-zero** (`pool_size: 0`): a workload holds no instance while
//!   idle; a request instantiates one and drops it afterwards. The ceiling is
//!   the host's own per-workload bookkeeping against the memory limit.
//!
//! Each flavour runs in a child process of its own, so neither inherits the
//! other's footprint. A child starts workloads in steps and, after each one,
//! sends requests from outside the host to workloads spread across the whole
//! set, then measures. It stops at the first of:
//!
//! - the memory guard: the engine's `max-process-memory` is the limit under
//!   test, and once its guard reads over the limit the run stops and reports
//!   what the guard refused;
//! - a start the host refuses, or most sampled requests failing (warm runs
//!   usually end here, at the pool);
//! - the footprint passing the limit by a quarter without the guard tripping,
//!   reported as the guard not holding, so a broken guard cannot take the
//!   machine with it;
//! - the component cap or the time budget, when set.
//!
//! This is a ramp, not a repeated measurement, so it does not use criterion.
//!
//! ```text
//! cargo xtask build-fixtures http-handler-p3
//! cargo bench -p wash-runtime --bench scale_density                # both
//! cargo bench -p wash-runtime --bench scale_density -- warm         # one
//! cargo bench -p wash-runtime --bench scale_density -- scale-to-zero
//! ```
//!
//! | Variable | Meaning | Default |
//! | --- | --- | --- |
//! | `WASH_SCALE_MEMORY_LIMIT` | the host's max-process-memory, e.g. `4GiB` | half of physical memory, at most 2 GiB |
//! | `WASH_SCALE_MAX_COMPONENTS` | stop after this many workloads | none |
//! | `WASH_SCALE_MAX_SECS` | stop a flavour after this long | `600` |
//! | `WASH_SCALE_STEP` | workloads started between measurements | `250` |
//! | `WASH_SCALE_START_CONCURRENCY` | concurrent starts | available parallelism |
//! | `WASH_SCALE_SAMPLE` | requests per step, spread across the set | `200` |
//! | `WASH_SCALE_CORE_INSTANCES` | pool slots | sized from address space and `vm.max_map_count` |
//! | `WASH_SCALE_FIXTURE` | `p3` or `p2` HTTP handler | `p3` |
//! | `WASH_SCALE_REPORT_JSON` | also write every flavour's results as JSON here | unset |
//!
//! CPU cannot be limited from inside the process; a run uses what the machine
//! gives it, and the report says how many cores that was.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use futures::{StreamExt as _, stream};

use wash_runtime::engine::Engine;
use wash_runtime::engine::host_memory::{
    self, HostMemoryBudgets, parse_bytes, pool_slot_bytes, render_bytes,
};
use wash_runtime::engine::pool_usage::{PoolUsage, pool_refusals};
use wash_runtime::engine::process_memory::process_footprint;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::types::{
    Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState,
};
use wash_runtime::wit::WitInterface;

/// Set on a child process: the flavour it runs, and where it writes its
/// [`Report`].
const CHILD_FLAVOUR: &str = "WASH_SCALE_CHILD_FLAVOUR";
const CHILD_REPORT: &str = "WASH_SCALE_CHILD_REPORT";

/// How far past the memory limit the footprint may go, with the guard still
/// not over, before the run gives up on it.
const GUARD_SLACK_PERCENT: u64 = 125;

fn main() -> Result<()> {
    if let Ok(flavour) = std::env::var(CHILD_FLAVOUR) {
        let flavour = Flavour::parse(&flavour)?;
        let path = std::env::var(CHILD_REPORT).context(CHILD_REPORT)?;
        let report = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run(flavour, Settings::from_env()?))?;
        std::fs::write(&path, serde_json::to_vec(&report)?)
            .with_context(|| format!("writing {path}"))?;
        return Ok(());
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    // `cargo test --benches` runs a harness-less bench without `--bench`, and
    // nextest lists it with `--list`. A ramp to the memory limit is no test;
    // only `cargo bench` runs it.
    if args.iter().any(|a| a == "--list") || !args.iter().any(|a| a == "--bench") {
        return Ok(());
    }
    let filters: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let flavours: Vec<Flavour> = Flavour::ALL
        .into_iter()
        .filter(|f| filters.is_empty() || filters.iter().any(|filter| f.name().contains(filter)))
        .collect();
    if flavours.is_empty() {
        bail!("no flavour matches {filters:?}; choose from warm, scale-to-zero");
    }
    // Read here too, so a bad variable fails before any child starts.
    let settings = Settings::from_env()?;

    let mut reports = Vec::new();
    for flavour in flavours {
        eprintln!("\n== {} ==", flavour.name());
        reports.push(run_child(flavour)?);
    }
    print_comparison(&settings, &reports);
    if let Some(path) = &settings.report_json {
        std::fs::write(path, serde_json::to_vec_pretty(&reports)?)
            .with_context(|| format!("writing {path}"))?;
    }
    Ok(())
}

/// Run one flavour in a fresh copy of this binary and read back its report.
fn run_child(flavour: Flavour) -> Result<Report> {
    let report = tempfile::NamedTempFile::new()?;
    let status = std::process::Command::new(std::env::current_exe()?)
        .env(CHILD_FLAVOUR, flavour.name())
        .env(CHILD_REPORT, report.path())
        .status()
        .with_context(|| format!("running the {} child", flavour.name()))?;
    if !status.success() {
        bail!("the {} run failed: {status}", flavour.name());
    }
    let bytes = std::fs::read(report.path())?;
    serde_json::from_slice(&bytes).with_context(|| format!("reading the {} report", flavour.name()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavour {
    Warm,
    ScaleToZero,
}

impl Flavour {
    const ALL: [Self; 2] = [Self::Warm, Self::ScaleToZero];

    fn name(self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::ScaleToZero => "scale-to-zero",
        }
    }

    fn parse(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|f| f.name() == name)
            .with_context(|| format!("unknown flavour {name}"))
    }

    fn pool_size(self) -> i32 {
        match self {
            Self::Warm => 1,
            Self::ScaleToZero => 0,
        }
    }
}

/// What a run was told, from the environment.
#[derive(Debug, Clone)]
struct Settings {
    report_json: Option<String>,
    memory_limit: u64,
    max_components: Option<usize>,
    max_duration: Option<Duration>,
    step: usize,
    start_concurrency: usize,
    sample: usize,
    core_instances: u32,
    fixture: &'static str,
}

impl Settings {
    fn from_env() -> Result<Self> {
        let var = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
        let number = |key: &str| -> Result<Option<usize>> {
            var(key)
                .map(|v| v.trim().parse().with_context(|| format!("{key}={v}")))
                .transpose()
        };
        let memory_limit = match var("WASH_SCALE_MEMORY_LIMIT") {
            Some(v) => parse_bytes(v.trim())
                .map_err(|e| anyhow::anyhow!("WASH_SCALE_MEMORY_LIMIT={v}: {e}"))?,
            None => (physical_memory()? / 2).min(2 << 30),
        };
        let core_instances = match number("WASH_SCALE_CORE_INSTANCES")? {
            Some(n) => u32::try_from(n).context("WASH_SCALE_CORE_INSTANCES")?,
            None => derived_core_instances(),
        };
        let fixture = match var("WASH_SCALE_FIXTURE").as_deref().map(str::trim) {
            None | Some("p3") => "http_handler_p3",
            Some("p2") => "http_handler_p2",
            Some(other) => bail!("WASH_SCALE_FIXTURE must be p2 or p3, not {other}"),
        };
        Ok(Self {
            report_json: var("WASH_SCALE_REPORT_JSON"),
            memory_limit,
            max_components: number("WASH_SCALE_MAX_COMPONENTS")?,
            max_duration: match number("WASH_SCALE_MAX_SECS")? {
                Some(0) => None,
                Some(secs) => Some(Duration::from_secs(secs as u64)),
                None => Some(Duration::from_secs(600)),
            },
            step: number("WASH_SCALE_STEP")?.unwrap_or(250).max(1),
            start_concurrency: number("WASH_SCALE_START_CONCURRENCY")?
                .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()))
                .max(1),
            sample: number("WASH_SCALE_SAMPLE")?.unwrap_or(200),
            core_instances,
            fixture,
        })
    }
}

fn physical_memory() -> Result<u64> {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};
    let system = System::new_with_specifics(
        RefreshKind::new().with_memory(MemoryRefreshKind::new().with_ram()),
    );
    match system.total_memory() {
        0 => bail!("cannot read physical memory; set WASH_SCALE_MEMORY_LIMIT"),
        total => Ok(total),
    }
}

/// Pool slots sized the way a large host would size them: half the address
/// space at wasmtime's fixed 4 GiB-plus-guard slot, and no more than the
/// kernel's mapping limit allows (about five mappings per slot).
fn derived_core_instances() -> u32 {
    let by_address_space = host_memory::usable_address_space()
        / 2
        / pool_slot_bytes(host_memory::WASMTIME_DEFAULT_HEAP_MEMORY);
    let by_map_count = std::fs::read_to_string("/proc/sys/vm/max_map_count")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map_or(u64::MAX, |limit| limit / 5);
    u32::try_from(by_address_space.min(by_map_count).max(1)).unwrap_or(u32::MAX)
}

fn fixture_bytes(name: &str) -> Result<bytes::Bytes> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/wasm")
        .join(format!("{name}.wasm"));
    let bytes = std::fs::read(&path).with_context(|| {
        format!(
            "missing {}; build it with `cargo xtask build-fixtures {}`",
            path.display(),
            name.replace('_', "-"),
        )
    })?;
    Ok(bytes.into())
}

/// A digest for the fixture, so every workload shares one compile the way
/// workloads pulled from one image do. Without one, each start compiles the
/// component again and the bench measures cranelift, not density.
fn fixture_digest(wasm: &[u8]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    wasm.hash(&mut hasher);
    format!("scale-density:{:016x}", hasher.finish())
}

fn http_interface(host_header: &str) -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: ["incoming-handler".to_string()].into_iter().collect(),
        version: Some(semver::Version::new(0, 2, 2)),
        config: HashMap::from([("host".to_string(), host_header.to_string())]),
        name: None,
    }
}

fn host_header(index: usize) -> String {
    format!("w{index}.scale.test")
}

/// Why a run stopped.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "reason", content = "detail", rename_all = "snake_case")]
enum Stop {
    /// `max-process-memory` tripped: the gate this bench measures.
    MemoryGuard {
        /// Memory growths the guard refused during the run.
        refused: u64,
    },
    /// The footprint went [`GUARD_SLACK_PERCENT`] of the limit without the
    /// guard tripping.
    GuardDidNotHold,
    /// The host refused a start; the message says what ran out.
    StartRefused(String),
    /// Most sampled requests failed.
    Requests(String),
    /// Reached `WASH_SCALE_MAX_COMPONENTS`.
    Cap,
    /// Ran out of `WASH_SCALE_MAX_SECS`.
    TimeBudget,
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MemoryGuard { refused } => {
                write!(f, "max-process-memory ({refused} growths refused)")
            }
            Self::GuardDidNotHold => f.write_str("footprint passed the limit, guard never tripped"),
            Self::StartRefused(message) => write!(f, "start refused: {message}"),
            Self::Requests(message) => write!(f, "requests: {message}"),
            Self::Cap => f.write_str("component cap"),
            Self::TimeBudget => f.write_str("time budget"),
        }
    }
}

/// One measurement, taken after a step of starts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Step {
    workloads: usize,
    elapsed_secs: f64,
    starts_per_sec: f64,
    footprint_bytes: u64,
    bytes_per_workload: u64,
    /// The engine's guard state: normal, high or over.
    guard: String,
    core_instances_in_use: u64,
    core_instances_total: u64,
    memories_in_use: u64,
    requests_ok: usize,
    requests_failed: usize,
    requests_per_sec: f64,
    latency_p50_ms: f64,
    latency_p99_ms: f64,
    /// Scale-to-zero only: how long until instances made by the sample were
    /// gone again.
    spin_down_ms: Option<f64>,
    /// Growths refused for the process limit, and instantiations refused by
    /// the pool, so far.
    memory_refusals: u64,
    pool_refusals: u64,
    /// The workload count the measurements so far say this host can hold.
    theoretical_limit: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Report {
    flavour: String,
    fixture: String,
    cores: usize,
    memory_limit_bytes: u64,
    core_instances: u32,
    baseline_footprint_bytes: u64,
    core_instances_per_workload: Option<u64>,
    /// The most workloads a step served at least half of its sample.
    served: usize,
    stop: Stop,
    steps: Vec<Step>,
}

async fn run(flavour: Flavour, settings: Settings) -> Result<Report> {
    let wasm = fixture_bytes(settings.fixture)?;
    let digest = fixture_digest(&wasm);

    // The guest budget is no tighter than the process limit, so the process
    // guard is what gates the run.
    let engine = Engine::builder()
        .with_host_memory(
            HostMemoryBudgets::resolve(
                Some(settings.memory_limit),
                None,
                Some(settings.core_instances),
            )
            .map_err(|e| anyhow::anyhow!(e))?,
        )
        .with_max_process_memory_bytes(settings.memory_limit)
        .build()?;
    let guard = engine
        .process_memory()
        .cloned()
        .context("the engine was built without a process memory guard")?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(ingress))
        .build()?
        .start()
        .await
        .context("host should start")?;

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(256)
        .timeout(Duration::from_secs(30))
        .build()?;
    let concurrency = settings.start_concurrency;
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let baseline = process_footprint().unwrap_or(0);
    let pool_baseline = engine.pool_usage();
    let baseline_in_use = pool_baseline.map_or(0, |u| u.core_instances.in_use);
    let memory_refusals_baseline = engine.guest_memory().process_refused();
    let pool_refusals_baseline = pool_refusals();
    let ceiling = settings.memory_limit / 100 * GUARD_SLACK_PERCENT;

    let run_started = Instant::now();
    let mut steps = Vec::new();
    let mut started = 0usize;
    let mut per_workload_core_instances = None;
    let stop = loop {
        if settings.max_components.is_some_and(|cap| started >= cap) {
            break Stop::Cap;
        }
        if settings
            .max_duration
            .is_some_and(|budget| run_started.elapsed() >= budget)
        {
            break Stop::TimeBudget;
        }
        let batch = settings
            .max_components
            .map_or(settings.step, |cap| settings.step.min(cap - started));
        let step_started = Instant::now();
        let refused =
            start_batch(&host, &wasm, &digest, flavour, started, batch, concurrency).await;
        let newly = match &refused {
            Ok(n) | Err((n, _)) => *n,
        };
        // Warm workloads are proven live, and their instance parked, by
        // answering one request each.
        let mut warm_failed = 0;
        if flavour == Flavour::Warm && newly > 0 {
            warm_failed = requests(&client, addr, started..started + newly, concurrency * 4)
                .await
                .failed;
        }
        started += newly;
        let step_secs = step_started.elapsed().as_secs_f64();

        let sample = spread(started, settings.sample);
        let sample_started = Instant::now();
        let mut answered = requests(&client, addr, sample.iter().copied(), concurrency * 4).await;
        let sample_secs = sample_started.elapsed().as_secs_f64();
        let spin_down_ms = match flavour {
            Flavour::Warm => None,
            Flavour::ScaleToZero => Some(wait_for_spin_down(&engine, baseline_in_use).await),
        };

        let footprint = process_footprint().unwrap_or(0);
        let usage = engine.pool_usage();
        let in_use = usage.map_or(0, |u| u.core_instances.in_use);
        if flavour == Flavour::Warm && per_workload_core_instances.is_none() && started > 0 {
            per_workload_core_instances =
                Some(in_use.saturating_sub(baseline_in_use) / started as u64).filter(|n| *n > 0);
        }
        let bytes_per_workload = footprint.saturating_sub(baseline) / started.max(1) as u64;
        let memory_refused = engine.guest_memory().process_refused() - memory_refusals_baseline;
        answered.latencies.sort_by(f64::total_cmp);
        let step = Step {
            workloads: started,
            elapsed_secs: run_started.elapsed().as_secs_f64(),
            starts_per_sec: newly as f64 / step_secs.max(f64::EPSILON),
            footprint_bytes: footprint,
            bytes_per_workload,
            guard: guard.state().as_str().to_string(),
            core_instances_in_use: in_use,
            core_instances_total: usage.map_or(0, |u| u.core_instances.total),
            memories_in_use: usage.map_or(0, |u| u.memories.in_use),
            requests_ok: answered.ok,
            requests_failed: answered.failed,
            requests_per_sec: (answered.ok + answered.failed) as f64
                / sample_secs.max(f64::EPSILON),
            latency_p50_ms: percentile(&answered.latencies, 0.50),
            latency_p99_ms: percentile(&answered.latencies, 0.99),
            spin_down_ms,
            memory_refusals: memory_refused,
            pool_refusals: pool_refusals() - pool_refusals_baseline,
            theoretical_limit: theoretical_limit(
                flavour,
                &settings,
                baseline,
                bytes_per_workload,
                per_workload_core_instances,
            ),
        };
        print_step(steps.is_empty(), &step);
        steps.push(step);

        // The guard first: a run it ends is the one this bench is for, and
        // the starts and requests that fail once it trips are its doing.
        if guard.is_over() {
            break Stop::MemoryGuard {
                refused: memory_refused,
            };
        }
        if footprint >= ceiling {
            break Stop::GuardDidNotHold;
        }
        if let Err((_, message)) = refused {
            break Stop::StartRefused(message);
        }
        if warm_failed > newly / 2 {
            break Stop::Requests(format!(
                "{warm_failed} of {newly} new warm workloads failed"
            ));
        }
        if !sample.is_empty() && answered.failed > sample.len() / 2 {
            break Stop::Requests(format!(
                "{} of {} sampled requests failed",
                answered.failed,
                sample.len()
            ));
        }
    };

    let served = steps
        .iter()
        .rev()
        .find(|s| s.requests_ok >= s.requests_failed)
        .map_or(0, |s| s.workloads);
    let report = Report {
        flavour: flavour.name().to_string(),
        fixture: settings.fixture.to_string(),
        cores,
        memory_limit_bytes: settings.memory_limit,
        core_instances: settings.core_instances,
        baseline_footprint_bytes: baseline,
        core_instances_per_workload: per_workload_core_instances,
        served,
        stop,
        steps,
    };
    eprintln!("stopped at {} workloads: {}", started, report.stop);
    // Not stopping the host: thousands of workload stops would only measure
    // teardown, and the child exits next.
    drop(host);
    Ok(report)
}

/// Start workloads `first..first + count`. Returns how many started, and,
/// if the host refused one, why.
async fn start_batch(
    host: &impl HostApi,
    wasm: &bytes::Bytes,
    digest: &str,
    flavour: Flavour,
    first: usize,
    count: usize,
    concurrency: usize,
) -> std::result::Result<usize, (usize, String)> {
    let results: Vec<std::result::Result<(), String>> = stream::iter(first..first + count)
        .map(|index| {
            let host_header = host_header(index);
            let request = WorkloadStartRequest {
                workload_id: uuid::Uuid::new_v4().to_string(),
                workload: Workload {
                    namespace: "scale".to_string(),
                    name: host_header.clone(),
                    annotations: HashMap::new(),
                    service: None,
                    components: vec![Component {
                        name: "handler".to_string(),
                        digest: Some(digest.to_string()),
                        bytes: wasm.clone(),
                        local_resources: LocalResources::default(),
                        pool_size: flavour.pool_size(),
                        ..Default::default()
                    }],
                    host_interfaces: vec![http_interface(&host_header)],
                    volumes: vec![],
                },
            };
            async move {
                match host.workload_start(request).await {
                    Ok(response)
                        if response.workload_status.workload_state == WorkloadState::Running =>
                    {
                        Ok(())
                    }
                    Ok(response) => Err(format!(
                        "{:?}: {}",
                        response.workload_status.workload_state, response.workload_status.message
                    )),
                    Err(e) => Err(format!("{e:#}")),
                }
            }
        })
        .buffered(concurrency)
        .collect()
        .await;
    // `buffered` keeps order, so everything before the first refusal started
    // and has a contiguous index the request sampler can address.
    let started = results.iter().take_while(|r| r.is_ok()).count();
    match results.into_iter().find_map(|r| r.err()) {
        Some(message) => Err((started, message)),
        None => Ok(started),
    }
}

struct Answered {
    ok: usize,
    failed: usize,
    /// Of each success, in milliseconds.
    latencies: Vec<f64>,
}

/// Send one GET to each workload index, from outside the host.
async fn requests(
    client: &reqwest::Client,
    addr: SocketAddr,
    indices: impl Iterator<Item = usize>,
    concurrency: usize,
) -> Answered {
    let results: Vec<Option<f64>> = stream::iter(indices)
        .map(|index| {
            let client = client.clone();
            async move {
                let started = Instant::now();
                let response = client
                    .get(format!("http://{addr}/"))
                    .header("HOST", host_header(index))
                    .send()
                    .await
                    .ok()?;
                if !response.status().is_success() {
                    return None;
                }
                response.bytes().await.ok()?;
                Some(started.elapsed().as_secs_f64() * 1000.0)
            }
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;
    let latencies: Vec<f64> = results.iter().flatten().copied().collect();
    Answered {
        ok: latencies.len(),
        failed: results.len() - latencies.len(),
        latencies,
    }
}

/// Up to `n` indices spread evenly across `0..count`, so a sample reaches the
/// oldest workloads as well as the newest.
fn spread(count: usize, n: usize) -> Vec<usize> {
    if count == 0 || n == 0 {
        return Vec::new();
    }
    let n = n.min(count);
    (0..n).map(|i| i * count / n).collect()
}

/// How long until the pool is back to the instances it held before the
/// sample, in milliseconds; capped at ten seconds.
async fn wait_for_spin_down(engine: &Engine, floor: u64) -> f64 {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if engine
            .pool_usage()
            .is_none_or(|u: PoolUsage| u.core_instances.in_use <= floor)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    started.elapsed().as_secs_f64() * 1000.0
}

/// The workload count the measurements so far say this host can hold: the
/// lower of what the pool fits (warm) and what the memory limit fits.
fn theoretical_limit(
    flavour: Flavour,
    settings: &Settings,
    baseline: u64,
    bytes_per_workload: u64,
    core_instances_per_workload: Option<u64>,
) -> usize {
    let by_memory = settings
        .memory_limit
        .saturating_sub(baseline)
        .checked_div(bytes_per_workload)
        .unwrap_or(u64::MAX);
    let by_pool = match (flavour, core_instances_per_workload) {
        (Flavour::Warm, Some(per)) => u64::from(settings.core_instances) / per.max(1),
        (Flavour::Warm, None) => u64::from(settings.core_instances),
        (Flavour::ScaleToZero, _) => u64::MAX,
    };
    usize::try_from(by_memory.min(by_pool)).unwrap_or(usize::MAX)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted.get(index).copied().unwrap_or(0.0)
}

fn print_step(header: bool, step: &Step) {
    if header {
        eprintln!(
            "{:>9} {:>8} {:>10} {:>9} {:>6} {:>13} {:>9} {:>7} {:>8} {:>8} {:>7} {:>9} {:>10}",
            "workloads",
            "starts/s",
            "footprint",
            "per-wkld",
            "guard",
            "core inst",
            "ok/fail",
            "req/s",
            "p50 ms",
            "p99 ms",
            "down ms",
            "mem/pool",
            "limit",
        );
    }
    eprintln!(
        "{:>9} {:>8.0} {:>10} {:>9} {:>6} {:>13} {:>9} {:>7.0} {:>8.1} {:>8.1} {:>7} {:>9} {:>10}",
        step.workloads,
        step.starts_per_sec,
        render_bytes(step.footprint_bytes),
        render_bytes(step.bytes_per_workload),
        step.guard,
        format!(
            "{}/{}",
            step.core_instances_in_use, step.core_instances_total
        ),
        format!("{}/{}", step.requests_ok, step.requests_failed),
        step.requests_per_sec,
        step.latency_p50_ms,
        step.latency_p99_ms,
        step.spin_down_ms
            .map_or_else(|| "-".to_string(), |ms| format!("{ms:.0}")),
        format!("{}/{}", step.memory_refusals, step.pool_refusals),
        step.theoretical_limit,
    );
}

/// The flavours side by side, one row per measure.
fn print_comparison(settings: &Settings, reports: &[Report]) {
    let Some(first) = reports.first() else {
        return;
    };
    eprintln!();
    eprintln!(
        "== {} on {} cores, max-process-memory {}, pool {} core instances ==",
        first.fixture,
        first.cores,
        render_bytes(settings.memory_limit),
        settings.core_instances,
    );
    let row = |label: &str, cell: &dyn Fn(&Report) -> String| {
        let cells: Vec<String> = reports.iter().map(|r| format!("{:>16}", cell(r))).collect();
        eprintln!("{label:<22}{}", cells.join(""));
    };
    let last = |r: &Report| r.steps.last().cloned();
    row("", &|r| r.flavour.clone());
    row("served workloads", &|r| r.served.to_string());
    row("theoretical limit", &|r| {
        last(r).map_or("-".into(), |s| s.theoretical_limit.to_string())
    });
    row("baseline footprint", &|r| {
        render_bytes(r.baseline_footprint_bytes)
    });
    row("final footprint", &|r| {
        last(r).map_or("-".into(), |s| render_bytes(s.footprint_bytes))
    });
    row("bytes per workload", &|r| {
        last(r).map_or("-".into(), |s| render_bytes(s.bytes_per_workload))
    });
    row("core inst / workload", &|r| {
        r.core_instances_per_workload
            .map_or("-".into(), |n| n.to_string())
    });
    row("starts/s (mean)", &|r| {
        let rates: Vec<f64> = r.steps.iter().map(|s| s.starts_per_sec).collect();
        format!(
            "{:.0}",
            rates.iter().sum::<f64>() / rates.len().max(1) as f64
        )
    });
    row("req/s (last step)", &|r| {
        last(r).map_or("-".into(), |s| format!("{:.0}", s.requests_per_sec))
    });
    row("p50 / p99 ms (last)", &|r| {
        last(r).map_or("-".into(), |s| {
            format!("{:.1} / {:.1}", s.latency_p50_ms, s.latency_p99_ms)
        })
    });
    row("spin-down ms (last)", &|r| {
        last(r)
            .and_then(|s| s.spin_down_ms)
            .map_or("-".into(), |ms| format!("{ms:.0}"))
    });
    row("refused mem / pool", &|r| {
        last(r).map_or("-".into(), |s| {
            format!("{} / {}", s.memory_refusals, s.pool_refusals)
        })
    });
    for report in reports {
        eprintln!("{}: stopped by {}", report.flavour, report.stop);
    }
}
