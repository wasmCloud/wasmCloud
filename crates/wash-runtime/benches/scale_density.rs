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
//! Beside what it served, it prints what each constraint (max-process-memory,
//! max-guest-memory, the pool, CPU) predicts the host can hold, ? when a
//! constraint cannot be estimated; `scale_density.md` beside this file says how each is derived.
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
//! | `WASH_SCALE_MEMORY_LIMIT` | the host's max-process-memory, e.g. `4GiB` | half of the machine/container budget, at most 2 GiB |
//! | `WASH_SCALE_GUEST_MEMORY` | max-guest-memory, the budget for guest linear memory as declared, resident or not | machine/container budget |
//! | `WASH_SCALE_MAX_COMPONENTS` | stop after this many workloads | none |
//! | `WASH_SCALE_MAX_SECS` | stop a flavour after this long | `600` |
//! | `WASH_SCALE_STEP` | workloads started between measurements | `250` |
//! | `WASH_SCALE_START_CONCURRENCY` | concurrent starts | available parallelism |
//! | `WASH_SCALE_SAMPLE` | requests per step, spread across the set | `200` |
//! | `WASH_SCALE_CORE_INSTANCES` | pool slots | sized from address space and `vm.max_map_count` |
//! | `WASH_SCALE_FIXTURE` | `p3` or `p2` HTTP handler | `p3` |
//! | `WASH_SCALE_CPUS` | cores the host may use; a list (`1,2,4`) runs each flavour at each | all |
//! | `WASH_SCALE_REPORT_JSON` | also write every flavour's results as JSON here | unset |
//!
//! A CPU limit pins the child to that many of the cores it may use, on Linux,
//! before it starts any thread the host uses, so tokio's workers, wasmtime's
//! compile pool and the memory sampler all inherit it; the host's runtime gets
//! that many workers too. Elsewhere there is no affinity to set, and only the
//! worker count is limited. The load generator runs on a runtime of its own,
//! built before the pin, so its requests still come from outside the limit.
//! CPU used is reported per step, without the load generator's on Linux and
//! with it elsewhere.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use futures::{StreamExt as _, stream};

use wash_runtime::engine::Engine;
use wash_runtime::engine::host_memory::HostMemoryBudgets;
use wash_runtime::engine::pool_usage::{PoolUsage, pool_refusals};
use wash_runtime::engine::process_memory::process_footprint;
use wash_runtime::host::http::{DynamicRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::types::{
    Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState,
};

#[path = "scale_density/estimate.rs"]
mod estimate;
#[path = "scale_density/load.rs"]
mod load;
#[path = "scale_density/report.rs"]
mod report;
#[path = "scale_density/settings.rs"]
mod settings;
use estimate::*;
use load::*;
use report::*;
use settings::*;

/// Set on a child process: the flavour it runs, its CPU limit, and where it
/// writes its [`Report`].
const CHILD_FLAVOUR: &str = "WASH_SCALE_CHILD_FLAVOUR";
const CHILD_CPUS: &str = "WASH_SCALE_CHILD_CPUS";
const CHILD_REPORT: &str = "WASH_SCALE_CHILD_REPORT";

/// How far past the memory limit the footprint may go, with the guard still
/// not over, before the run gives up on it.
const GUARD_SLACK_PERCENT: u64 = 125;

fn main() -> Result<()> {
    if let Ok(flavour) = std::env::var(CHILD_FLAVOUR) {
        return child_main(Flavour::parse(&flavour)?);
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
    for &flavour in &flavours {
        settings.validate_flavour(flavour)?;
    }

    let mut reports = Vec::new();
    for &cpus in &settings.cpus {
        for &flavour in &flavours {
            eprintln!("\n== {} ==", label(flavour.name(), cpus));
            reports.push(run_child(flavour, cpus)?);
        }
    }
    print_comparison(&settings, &reports);
    if let Some(path) = &settings.report_json {
        std::fs::write(path, serde_json::to_vec_pretty(&reports)?)
            .with_context(|| format!("writing {path}"))?;
    }
    Ok(())
}

/// Run one flavour in a fresh copy of this binary and read back its report.
fn run_child(flavour: Flavour, cpus: Option<usize>) -> Result<Report> {
    let name = label(flavour.name(), cpus);
    let report = tempfile::NamedTempFile::new()?;
    let mut child = std::process::Command::new(std::env::current_exe()?);
    child
        .env(CHILD_FLAVOUR, flavour.name())
        .env(CHILD_REPORT, report.path());
    if let Some(cpus) = cpus {
        child.env(CHILD_CPUS, cpus.to_string());
    }
    let status = child
        .status()
        .with_context(|| format!("running the {name} child"))?;
    if !status.success() {
        bail!("the {name} run failed: {status}");
    }
    let bytes = std::fs::read(report.path())?;
    serde_json::from_slice(&bytes).with_context(|| format!("reading the {name} report"))
}

/// A flavour, and its CPU limit when it has one: `warm`, `warm@2cpu`.
fn label(flavour: &str, cpus: Option<usize>) -> String {
    match cpus {
        Some(n) => format!("{flavour}@{n}cpu"),
        None => flavour.to_string(),
    }
}

fn child_main(flavour: Flavour) -> Result<()> {
    // The host's own logs, only when asked for: at the default level a ramp
    // to the limit is thousands of lines.
    if std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .init();
    }
    let path = std::env::var(CHILD_REPORT).context(CHILD_REPORT)?;
    let cpu_limit = std::env::var(CHILD_CPUS)
        .ok()
        .map(|v| {
            v.parse::<usize>()
                .with_context(|| format!("{CHILD_CPUS}={v}"))
        })
        .transpose()?;

    // Built before the pin, so its threads keep every core and the requests
    // they send are not charged to the host.
    let load = Arc::new(LoadThreads::default());
    let load_runtime = {
        let load = Arc::clone(&load);
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("scale-load")
            .on_thread_start(move || load.register_current())
            .enable_all()
            .build()?
    };
    // Every worker has started once `build` returns; wait for them to say so.
    load.wait_for(load_runtime.metrics().num_workers());

    let pinned = cpu_limit.map(pin_to_cpus).transpose()?.unwrap_or(false);
    let mut host_runtime = tokio::runtime::Builder::new_multi_thread();
    if let Some(cpus) = cpu_limit {
        host_runtime.worker_threads(cpus);
    }
    let cpu = CpuLimit {
        limit: cpu_limit,
        pinned,
        cores: cpu_limit
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get())),
    };
    let report = host_runtime.enable_all().build()?.block_on(run(
        flavour,
        Settings::from_env()?,
        cpu,
        Load::new(load_runtime.handle().clone(), load)?,
    ))?;
    std::fs::write(&path, serde_json::to_vec(&report)?)
        .with_context(|| format!("writing {path}"))?;
    Ok(())
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
    /// max-guest-memory refused growth first: the budget for guest memory
    /// as declared, which warm instances fill before the footprint does.
    GuestBudget { refused: u64 },
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
            Self::GuestBudget { refused } => {
                write!(f, "max-guest-memory ({refused} growths refused)")
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
    memories_total: u64,
    /// Workloads this step started, and the requests its sample sent.
    started_in_step: usize,
    sample_requests: usize,
    requests_ok: usize,
    requests_failed: usize,
    warmup_failed: usize,
    requests_per_sec: f64,
    latency_p50_ms: Option<f64>,
    latency_p99_ms: Option<f64>,
    /// Scale-to-zero only: how long until instances made by the sample were
    /// gone again.
    spin_down_ms: Option<f64>,
    /// Growths refused for the process limit, and instantiations refused by
    /// the pool, so far.
    memory_refusals: u64,
    pool_refusals: u64,
    /// Guest memory charged now, and growths the guest budget refused so far.
    guest_memory_bytes: u64,
    guest_refusals: u64,
    /// Workload starts the memory guard refused so far.
    refused_starts: u64,
    /// Nothing was refused during this step and its new workloads served:
    /// only clean steps say what a workload costs (see [`Limits`]).
    clean: bool,
    /// CPU the host used during the step, and that as a share of the cores
    /// it may use over the step's wall time.
    cpu_secs: Option<f64>,
    cpu_percent: Option<f64>,
    /// Of that, starting the step's workloads (with a warm one's first
    /// request), and serving its sample (with a scale-to-zero spin-down).
    start_cpu_secs: Option<f64>,
    sample_cpu_secs: Option<f64>,
    /// The workload count the measurements so far say this host can hold,
    /// the lowest measured bound in [`Limits`]; `None` when no bound is known.
    theoretical_limit: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Report {
    flavour: String,
    /// The flavour and its CPU limit, as the comparison heads its column.
    label: String,
    fixture: String,
    /// Cores the host may use: the CPU limit, or every core.
    cores: usize,
    cpu_limit: Option<usize>,
    /// Whether the limit was held by affinity, not just tokio's worker count.
    cpu_pinned: bool,
    /// Whether CPU figures leave out the load generator.
    cpu_host_only: bool,
    cpu_secs_total: Option<f64>,
    memory_limit_bytes: u64,
    guest_memory_bytes: u64,
    core_instances: u32,
    baseline_footprint_bytes: u64,
    core_instances_per_workload: Option<u64>,
    /// Workloads at the last clean step: every one started and served, and
    /// nothing refused.
    served: usize,
    limits: Limits,
    stop: Stop,
    steps: Vec<Step>,
}

async fn run(flavour: Flavour, settings: Settings, cpu: CpuLimit, load: Load) -> Result<Report> {
    settings.validate_flavour(flavour)?;
    let wasm = fixture_bytes(settings.fixture)?;
    let digest = fixture_digest(&wasm);

    // The guest budget charges each linear memory at its declared size, resident
    // or not, so a warm workload costs it several times what it adds to the
    // footprint. Left at the machine/container budget, it leaves the process guard to gate
    // the run; set lower, the run shows where it binds instead.
    let engine = Engine::builder()
        .with_host_memory(
            HostMemoryBudgets::resolve(
                Some(settings.guest_memory),
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
    // Routed by Host header, as a real host routes: `DevRouter` would send
    // every request to the newest workload and never reach the rest.
    let ingress = Ingress::new(DynamicRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(ingress))
        .build()?
        .start()
        .await
        .context("host should start")?;

    let concurrency = settings.start_concurrency.unwrap_or(cpu.cores);
    let baseline = process_footprint().unwrap_or(0);
    let (cpu_baseline, cpu_host_only) = host_cpu(&load.threads);
    let mut cpu_before = cpu_baseline;
    let pool_baseline = engine.pool_usage();
    let baseline_in_use = pool_baseline.map_or(0, |u| u.core_instances.in_use);
    let memory_refusals_baseline = engine.guest_memory().process_refused();
    let guest_refusals_baseline = engine.guest_memory().refused();
    let pool_refusals_baseline = pool_refusals();
    let ceiling = settings.memory_limit / 100 * GUARD_SLACK_PERCENT;
    let mut refusals_before = (0, 0, 0, 0);

    let run_started = Instant::now();
    let mut steps = Vec::new();
    let mut started = 0usize;
    let mut running = Vec::new();
    let mut next_index = 0;
    let mut per_workload_core_instances = None;
    let mut serving = bench_tools::density::ServingEvidence::default();
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
        let starts = start_batch(
            &host,
            &wasm,
            &digest,
            flavour,
            settings.fixture,
            next_index..next_index + batch,
            concurrency,
        )
        .await;
        next_index += batch;
        let newly = starts.running.len();
        // Warm workloads are proven live, and their instance parked, by
        // answering one request each.
        let mut warm_failed = 0;
        if flavour == Flavour::Warm && newly > 0 {
            warm_failed = load
                .send(addr, starts.running.clone(), concurrency * 4)
                .await
                .failed;
        }
        running.extend(starts.running);
        started = running.len();
        let step_secs = step_started.elapsed().as_secs_f64();
        let (cpu_started, _) = host_cpu(&load.threads);

        let sample: Vec<_> = spread(started, settings.sample)
            .into_iter()
            .filter_map(|position| running.get(position).copied())
            .collect();
        let sample_started = Instant::now();
        let answered = load.send(addr, sample.clone(), concurrency * 4).await;
        let sample_secs = sample_started.elapsed().as_secs_f64();
        let spin_down_ms = match flavour {
            Flavour::Warm => None,
            Flavour::ScaleToZero => Some(wait_for_spin_down(&engine, baseline_in_use).await),
        };

        let footprint = process_footprint().unwrap_or(0);
        let usage = engine.pool_usage();
        let in_use = usage.map_or(0, |u| u.core_instances.in_use);
        if flavour == Flavour::Warm
            && per_workload_core_instances.is_none()
            && started > 0
            && warm_failed == 0
        {
            per_workload_core_instances =
                Some(in_use.saturating_sub(baseline_in_use) / started as u64).filter(|n| *n > 0);
        }
        let bytes_per_workload = footprint.saturating_sub(baseline) / started.max(1) as u64;
        let memory_refused = engine.guest_memory().process_refused() - memory_refusals_baseline;
        let guest_refused = engine.guest_memory().refused() - guest_refusals_baseline;
        let (cpu_now, _) = host_cpu(&load.threads);
        let between = |from: Option<Duration>, to: Option<Duration>| {
            to.zip(from)
                .and_then(|(to, from)| to.checked_sub(from).map(|d| d.as_secs_f64()))
        };
        let cpu_secs = between(cpu_before, cpu_now);
        let start_cpu_secs = between(cpu_before, cpu_started);
        let sample_cpu_secs = between(cpu_started, cpu_now);
        cpu_before = cpu_now;
        let wall_secs = step_started.elapsed().as_secs_f64().max(f64::EPSILON);
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
            memories_total: usage.map_or(0, |u| u.memories.total),
            started_in_step: newly,
            sample_requests: sample.len(),
            requests_ok: answered.ok,
            requests_failed: answered.failed,
            warmup_failed: warm_failed,
            requests_per_sec: answered.ok as f64 / sample_secs.max(f64::EPSILON),
            latency_p50_ms: percentile(&answered.latencies, 0.50),
            latency_p99_ms: percentile(&answered.latencies, 0.99),
            spin_down_ms,
            memory_refusals: memory_refused,
            pool_refusals: pool_refusals() - pool_refusals_baseline,
            guest_memory_bytes: engine.guest_memory().in_use(),
            guest_refusals: guest_refused,
            refused_starts: guard.refused_starts(),
            clean: false,
            cpu_secs,
            cpu_percent: cpu_secs.map(|secs| secs / (wall_secs * cpu.cores as f64) * 100.0),
            start_cpu_secs,
            sample_cpu_secs,
            theoretical_limit: None,
        };
        let refusals_now = (
            memory_refused,
            guest_refused,
            pool_refusals() - pool_refusals_baseline,
            guard.refused_starts(),
        );
        let mut step = step;
        let served = serving.observe(step.requests_ok, step.requests_failed, warm_failed);
        step.clean = refusals_now == refusals_before && starts.first_error.is_none() && served;
        refusals_before = refusals_now;
        steps.push(step);
        let so_far = Limits::from_steps(&settings, cpu.cores, baseline, &pool_baseline, &steps);
        let first = steps.len() == 1;
        let step = steps.last_mut().expect("just pushed");
        step.theoretical_limit = so_far.binding().1;
        print_step(first, step);

        // The guard first: a run it ends is the one this bench is for, and
        // the starts and requests that fail once it trips are its doing.
        if guard.is_over() {
            break Stop::MemoryGuard {
                refused: memory_refused,
            };
        }
        if guest_refused > 0 {
            break Stop::GuestBudget {
                refused: guest_refused,
            };
        }
        if footprint >= ceiling {
            break Stop::GuardDidNotHold;
        }
        if let Some(message) = starts.first_error {
            break Stop::StartRefused(message);
        }
        if warm_failed > 0 {
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
        .find(|s| s.clean)
        .map_or(0, |s| s.workloads);
    let report = Report {
        flavour: flavour.name().to_string(),
        label: label(flavour.name(), cpu.limit),
        fixture: settings.fixture.to_string(),
        cores: cpu.cores,
        cpu_limit: cpu.limit,
        cpu_pinned: cpu.pinned,
        cpu_host_only,
        cpu_secs_total: host_cpu(&load.threads)
            .0
            .zip(cpu_baseline)
            .map(|(now, before)| now.saturating_sub(before).as_secs_f64()),
        memory_limit_bytes: settings.memory_limit,
        guest_memory_bytes: settings.guest_memory,
        core_instances: settings.core_instances,
        baseline_footprint_bytes: baseline,
        core_instances_per_workload: per_workload_core_instances,
        served,
        limits: Limits::from_steps(&settings, cpu.cores, baseline, &pool_baseline, &steps),
        stop,
        steps,
    };
    eprintln!("stopped at {} workloads: {}", started, report.stop);
    // Not stopping the host: thousands of workload stops would only measure
    // teardown, and the child exits next.
    drop(host);
    Ok(report)
}

#[derive(Debug, Default)]
struct StartedBatch {
    running: Vec<usize>,
    first_error: Option<String>,
}

/// Drain every submitted start, including successes after a failed index.
async fn collect_starts<F>(
    requests: impl futures::Stream<Item = F>,
    concurrency: usize,
) -> StartedBatch
where
    F: std::future::Future<Output = (usize, std::result::Result<(), String>)>,
{
    let results = requests
        .buffered(concurrency.max(1))
        .collect::<Vec<_>>()
        .await;
    let mut batch = StartedBatch::default();
    for (index, result) in results {
        match result {
            Ok(()) => batch.running.push(index),
            Err(error) if batch.first_error.is_none() => batch.first_error = Some(error),
            Err(_) => {}
        }
    }
    batch
}

/// Start workloads `first..first + count`, retaining their actual indices.
async fn start_batch(
    host: &impl HostApi,
    wasm: &bytes::Bytes,
    digest: &str,
    flavour: Flavour,
    fixture: &str,
    indices: std::ops::Range<usize>,
    concurrency: usize,
) -> StartedBatch {
    let requests = stream::iter(indices).map(|index| {
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
                host_interfaces: vec![http_interface(&host_header, fixture)],
                volumes: vec![],
            },
        };
        async move {
            let result = match host.workload_start(request).await {
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
            };
            (index, result)
        }
    });
    collect_starts(requests, concurrency).await
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

use bench_tools::density::{percentile, spread};

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

#[cfg(test)]
mod tests {
    // These run from `tests/scale_density.rs`. `cargo test` also builds this
    // harnessless bench with `cfg(test)` but drops its `#[test]` functions,
    // which leaves the import unused there.
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn fixture_binding_matches_the_selected_http_version() {
        let p3 = http_interface("w0.test", "http_handler_p3");
        assert!(p3.interfaces.contains("handler"));
        assert_eq!(p3.version, Some(semver::Version::new(0, 3, 0)));
        let p2 = http_interface("w0.test", "http_handler_p2");
        assert!(p2.interfaces.contains("incoming-handler"));
        assert_eq!(p2.version, Some(semver::Version::new(0, 2, 2)));
    }

    #[test]
    fn missing_cpu_does_not_inflate_rates_or_claim_infinite_capacity() {
        assert_eq!(measured_rate(2, [(None, 1000)].into_iter()), None);
        assert_eq!(measured_rate(2, [(Some(0.0), 1000)].into_iter()), None);
        assert_eq!(
            measured_rate(2, [(None, 1000), (Some(2.0), 10)].into_iter()),
            Some(10.0)
        );
        assert_eq!(or_unknown(None), "?");
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(
            serde_json::to_value(CpuCapacity::IdleUnbounded).unwrap(),
            "idle-unbounded"
        );
    }

    #[test]
    fn p2_cannot_be_reported_as_a_warm_pool() -> Result<()> {
        let settings =
            Settings::from_values(|key| (key == "WASH_SCALE_FIXTURE").then(|| "p2".into()))?;
        assert!(settings.validate_flavour(Flavour::Warm).is_err());
        settings.validate_flavour(Flavour::ScaleToZero)?;
        Settings::from_values(|_| None)?.validate_flavour(Flavour::Warm)?;
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn partial_start_batches_keep_later_successes() {
        let completed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let requests = stream::iter(0..3).map(|index| {
            let completed = Arc::clone(&completed);
            async move {
                tokio::time::sleep(Duration::from_secs(3 - index as u64)).await;
                completed.lock().unwrap().push(index);
                (
                    index,
                    if index == 1 {
                        Err("start 1 refused".into())
                    } else {
                        Ok(())
                    },
                )
            }
        });
        let batch = collect_starts(requests, 3).await;
        assert_eq!(*completed.lock().unwrap(), vec![2, 1, 0]);
        assert_eq!(batch.running, vec![0, 2]);
        assert_eq!(batch.first_error.as_deref(), Some("start 1 refused"));
        let sampled: Vec<_> = spread(batch.running.len(), 2)
            .into_iter()
            .filter_map(|position| batch.running.get(position).copied())
            .collect();
        assert_eq!(sampled, vec![0, 2]);
    }

    #[test]
    fn zero_samples_cannot_claim_served_capacity() -> Result<()> {
        let zero = Settings::from_values(|key| (key == "WASH_SCALE_SAMPLE").then(|| "0".into()));
        assert!(zero.is_err());
        let one = Settings::from_values(|key| (key == "WASH_SCALE_SAMPLE").then(|| "1".into()))?;
        assert_eq!(one.sample, 1);
        Ok(())
    }
}
