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
//! | `WASH_SCALE_GUEST_MEMORY_MODE` | `enforce`, `count` or `growth`: how the guest budget is applied; a list (`enforce,growth`) runs each flavour in each | `enforce` |
//! | `WASH_SCALE_INITIAL_GUEST_MEMORY` | under `growth`, the cap on initial memory left uncharged | four times `WASH_SCALE_GUEST_MEMORY` |
//! | `WASH_SCALE_MAX_COMPONENTS` | stop after this many workloads | none |
//! | `WASH_SCALE_MAX_SECS` | stop a flavour after this long | `600` |
//! | `WASH_SCALE_STEP` | workloads started between measurements | `250` |
//! | `WASH_SCALE_START_CONCURRENCY` | concurrent starts | available parallelism |
//! | `WASH_SCALE_SAMPLE` | requests per step, spread across the set | `200` |
//! | `WASH_SCALE_MAX_MEMORIES` | pool linear memories, the count that sizes the pool (`WASH_SCALE_CORE_INSTANCES` is an older name for it) | what the runtime derives from address space and `vm.max_map_count` |
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

use bench_tools::density::{Report, Step, percentile, print_comparison, print_step, spread};
use wash_runtime::engine::Engine;
use wash_runtime::engine::guest_memory::GuestMemoryMode;
use wash_runtime::engine::host_memory::{HostMemoryBudgets, render_bytes};
use wash_runtime::engine::pool_usage::{PoolUsage, pool_refusals};
use wash_runtime::engine::process_memory::process_footprint;
use wash_runtime::host::http::{DynamicRouter, Ingress};
use wash_runtime::host::{Host, HostApi, HostBuilder};
use wash_runtime::types::{
    Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState,
};

#[path = "scale_density/estimate.rs"]
mod estimate;
#[path = "scale_density/load.rs"]
mod load;
#[path = "scale_density/settings.rs"]
mod settings;
use estimate::*;
use load::*;
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
        return child_main(parse_flavour(&flavour)?);
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
    for &mode in &settings.guest_modes {
        for &cpus in &settings.cpus {
            for &flavour in &flavours {
                eprintln!("\n== {} ==", report_label(flavour.name(), cpus, mode));
                reports.push(run_child(flavour, cpus, mode)?);
            }
        }
    }
    print_comparison(&reports);
    if let Some(path) = &settings.report_json {
        std::fs::write(path, serde_json::to_vec_pretty(&reports)?)
            .with_context(|| format!("writing {path}"))?;
    }
    Ok(())
}

/// Run one flavour in one guest memory mode in a fresh copy of this binary,
/// and read back its report.
fn run_child(flavour: Flavour, cpus: Option<usize>, mode: GuestMemoryMode) -> Result<Report> {
    let name = report_label(flavour.name(), cpus, mode);
    let report = tempfile::NamedTempFile::new()?;
    let mut child = std::process::Command::new(std::env::current_exe()?);
    child
        .env(CHILD_FLAVOUR, flavour.name())
        .env(CHILD_REPORT, report.path())
        // One mode per child, in place of the list the parent was given.
        .env(GUEST_MEMORY_MODE, mode.as_str());
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

/// Preserve the default series name, and keep experiments in other memory
/// modes out of that series when `bench-tools jsonl` publishes the report.
fn report_label(flavour: &str, cpus: Option<usize>, mode: GuestMemoryMode) -> String {
    let label = label(flavour, cpus);
    if mode == GuestMemoryMode::Enforce {
        label
    } else {
        format!("{label}-{}", mode.as_str())
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
    /// Growth mode's cap on uncharged first allocations was reached.
    InitialGuestBudget { refused: u64 },
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
            Self::InitialGuestBudget { refused } => {
                write!(
                    f,
                    "max-initial-guest-memory ({refused} allocations refused)"
                )
            }
            Self::GuardDidNotHold => f.write_str("footprint passed the limit, guard never tripped"),
            Self::StartRefused(message) => write!(f, "start refused: {message}"),
            Self::Requests(message) => write!(f, "requests: {message}"),
            Self::Cap => f.write_str("component cap"),
            Self::TimeBudget => f.write_str("time budget"),
        }
    }
}

// A missing reading cannot support either a capacity estimate or the guard's
// overshoot check. End the run rather than report it as a measured zero.
fn measured_footprint(reading: Option<u64>, phase: &str) -> Result<u64> {
    reading.with_context(|| {
        format!("process footprint unavailable during {phase}; density run stopped")
    })
}

async fn run(flavour: Flavour, settings: Settings, cpu: CpuLimit, load: Load) -> Result<Report> {
    settings.validate_flavour(flavour)?;
    let wasm = fixture_bytes(settings.fixture)?;
    let digest = fixture_digest(&wasm);

    // In enforce mode (the default) the guest budget charges each linear memory
    // at its declared size, resident or not, so a warm workload costs it several
    // times what it adds to the footprint; growth mode charges only growth, and
    // count mode refuses nothing. Left at the machine/container budget, the
    // budget leaves the process guard to gate the run; set lower, the run shows
    // where it binds instead.
    let mut builder = Engine::builder();
    if let Some(cap) = settings.initial_guest_memory {
        builder = builder.with_max_initial_guest_memory_bytes(cap);
    }
    let engine = builder
        .with_host_memory(
            // The bench's count is pool slots, which are memories.
            HostMemoryBudgets::resolve(
                Some(settings.guest_memory),
                None,
                Some(settings.max_memories),
                None,
            )
            .map_err(|e| anyhow::anyhow!(e))?,
        )
        .with_max_process_memory_bytes(settings.memory_limit)
        .with_guest_memory_mode(settings.guest_mode)
        .build()?;
    // The cap the engine actually applies, so the estimate cannot drift from it.
    let settings = Settings {
        initial_guest_memory: Some(engine.guest_memory().initial_cap()),
        ..settings
    };
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
    let baseline = measured_footprint(process_footprint(), "baseline")?;
    let (cpu_baseline, cpu_host_only) = host_cpu(&load.threads);
    let mut cpu_before = cpu_baseline;
    let pool_baseline = engine.pool_usage();
    let baseline_in_use = pool_baseline.map_or(0, |u| u.core_instances.in_use);
    let memory_refusals_baseline = engine.guest_memory().process_refused();
    let guest_refusals_baseline = engine.guest_memory().refused();
    let initial_refusals_baseline = engine.guest_memory().initial_refused();
    let pool_refusals_baseline = pool_refusals();
    let ceiling = settings.memory_limit / 100 * GUARD_SLACK_PERCENT;
    let mut refusals_before = [0; 5];

    let run_started = Instant::now();
    let mut steps = Vec::new();
    let mut started = 0usize;
    let mut running = Vec::new();
    let mut next_index = 0;
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

        let footprint = measured_footprint(process_footprint(), "step")?;
        let usage = engine.pool_usage();
        let in_use = usage.map_or(0, |u| u.core_instances.in_use);
        let bytes_per_workload = footprint.saturating_sub(baseline) / started.max(1) as u64;
        let memory_refused = engine.guest_memory().process_refused() - memory_refusals_baseline;
        let guest_refused = engine.guest_memory().refused() - guest_refusals_baseline;
        let initial_refused = engine.guest_memory().initial_refused() - initial_refusals_baseline;
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
        let refusals_now = [
            memory_refused,
            guest_refused,
            initial_refused,
            pool_refusals() - pool_refusals_baseline,
            guard.refused_starts(),
        ];
        let refused_in_step = refusals_now
            .iter()
            .zip(refusals_before)
            .map(|(now, before)| now - before)
            .sum::<u64>()
            + u64::from(starts.first_error.is_some());
        refusals_before = refusals_now;
        let served = serving.observe(answered.ok, answered.failed, warm_failed);
        steps.push(Step {
            workloads: started,
            started: newly,
            starts_per_sec: newly as f64 / step_secs.max(f64::EPSILON),
            start_p99_ms: None,
            refused: refused_in_step,
            footprint_bytes: Some(footprint),
            bytes_per_workload: Some(bytes_per_workload),
            guest_memory_bytes: Some(engine.guest_memory().in_use()),
            initial_memory_bytes: Some(engine.guest_memory().initial_uncharged()),
            core_instances_in_use: Some(in_use),
            core_instances_total: usage.map(|u| u.core_instances.total),
            memories_in_use: usage.map(|u| u.memories.in_use),
            memories_total: usage.map(|u| u.memories.total),
            requests_ok: answered.ok,
            requests_failed: answered.failed,
            warmup_failed: warm_failed,
            requests_per_sec: answered.ok as f64 / sample_secs.max(f64::EPSILON),
            request_p50_ms: percentile(&answered.latencies, 0.50),
            request_p99_ms: percentile(&answered.latencies, 0.99),
            spin_down_ms,
            cpu_percent: cpu_secs.map(|secs| secs / (wall_secs * cpu.cores as f64) * 100.0),
            start_cpu_secs,
            sample_cpu_secs,
            clean: refused_in_step == 0 && served,
            theoretical_limit: None,
        });
        let so_far = Estimate::from_steps(&settings, cpu.cores, baseline, &pool_baseline, &steps);
        let first = steps.len() == 1;
        let step = steps.last_mut().expect("just pushed");
        step.theoretical_limit = so_far.binding();
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
        if initial_refused > 0 {
            break Stop::InitialGuestBudget {
                refused: initial_refused,
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

    let estimate = Estimate::from_steps(&settings, cpu.cores, baseline, &pool_baseline, &steps);
    let report = Report {
        bench: "scale_density".into(),
        flavour: Some(flavour),
        label: report_label(flavour.name(), cpu.limit, settings.guest_mode),
        subject: format!(
            "{}, max-guest-memory {}, pool {} memories, {}{}",
            settings.fixture,
            render_bytes(settings.guest_memory),
            settings.max_memories,
            match (cpu.limit, cpu.pinned) {
                (Some(n), true) => format!("{n} pinned cores"),
                (Some(n), false) => format!("{n} workers"),
                (None, _) => format!("{} cores", cpu.cores),
            },
            if cpu_host_only {
                ""
            } else {
                ", CPU includes the load generator's"
            },
        ),
        guest_memory_mode: Some(settings.guest_mode.as_str().into()),
        cores: cpu.cores,
        memory_limit_bytes: settings.memory_limit,
        baseline_footprint_bytes: Some(baseline),
        limits: estimate.limits,
        marginal_bytes_per_workload: estimate.marginal_bytes_per_workload,
        cpu_max_starts_per_sec: estimate.max_starts_per_sec,
        cpu_max_requests_per_sec: estimate.max_requests_per_sec,
        cpu_secs_total: host_cpu(&load.threads)
            .0
            .zip(cpu_baseline)
            .map(|(now, before)| now.saturating_sub(before).as_secs_f64()),
        stop: stop.to_string(),
        steps,
    };
    eprintln!("stopped at {started} workloads: {}", report.stop);
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
    host: &Arc<Host>,
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
        // Spawned, not polled here: `buffered` drives every future it holds
        // from this one task, so unspawned starts all run on the thread that
        // polls it and the host's other workers sit idle. Each start runs on
        // a worker of its own; `buffered` still caps how many are in flight.
        let host = Arc::clone(host);
        async move {
            let started = tokio::spawn(async move { host.workload_start(request).await })
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!("start task failed: {e}")));
            let result = match started {
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
    fn missing_memory_readings_stop_instead_of_claiming_zero() -> Result<()> {
        for phase in ["baseline", "step"] {
            let error = measured_footprint(None, phase).unwrap_err().to_string();
            assert!(error.contains(phase), "{error}");
            assert!(error.contains("density run stopped"), "{error}");
        }
        assert_eq!(measured_footprint(Some(1024), "baseline")?, 1024);
        assert_eq!(measured_footprint(Some(0), "step")?, 0);
        Ok(())
    }

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
    }

    #[test]
    fn observational_guest_budgets_do_not_limit_capacity() -> Result<()> {
        for (mode, enforced) in [("enforce", true), ("growth", true), ("count", false)] {
            let settings = Settings::from_values(|key| match key {
                "WASH_SCALE_MEMORY_LIMIT" => Some("1GiB".into()),
                "WASH_SCALE_GUEST_MEMORY" => Some("10MiB".into()),
                "WASH_SCALE_MAX_MEMORIES" => Some("1000".into()),
                "WASH_SCALE_GUEST_MEMORY_MODE" => Some(mode.into()),
                _ => None,
            })?;
            let step = Step {
                workloads: 100,
                clean: true,
                footprint_bytes: Some(100 << 20),
                guest_memory_bytes: Some(100 << 20),
                ..Default::default()
            };
            let estimate = Estimate::from_steps(&settings, 1, 0, &None, &[step]);
            assert_eq!(estimate.limits.contains_key("max-guest-memory"), enforced);
            assert_eq!(estimate.binding(), Some(if enforced { 10 } else { 1024 }));
        }
        Ok(())
    }

    #[test]
    fn growth_mode_is_bound_by_its_initial_cap() -> Result<()> {
        let settings = Settings::from_values(|key| match key {
            "WASH_SCALE_MEMORY_LIMIT" => Some("1GiB".into()),
            "WASH_SCALE_GUEST_MEMORY" => Some("10MiB".into()),
            "WASH_SCALE_MAX_MEMORIES" => Some("1000".into()),
            "WASH_SCALE_GUEST_MEMORY_MODE" => Some("growth".into()),
            "WASH_SCALE_INITIAL_GUEST_MEMORY" => Some("40MiB".into()),
            _ => None,
        })?;
        let step = Step {
            workloads: 100,
            clean: true,
            footprint_bytes: Some(100 << 20),
            guest_memory_bytes: Some(0),
            initial_memory_bytes: Some(400 << 20),
            ..Default::default()
        };
        let estimate = Estimate::from_steps(&settings, 1, 0, &None, &[step]);
        // A 40 MiB cap, and each workload leaves 4 MiB uncharged.
        assert_eq!(estimate.limits["max-initial-guest-memory"], Some(10));
        assert_eq!(estimate.binding(), Some(10));
        Ok(())
    }

    #[test]
    fn a_list_of_guest_memory_modes_runs_each_in_turn() -> Result<()> {
        let settings = |modes: Option<&str>| {
            Settings::from_values(|key| match key {
                "WASH_SCALE_MEMORY_LIMIT" | "WASH_SCALE_GUEST_MEMORY" => Some("1GiB".into()),
                "WASH_SCALE_MAX_MEMORIES" => Some("1000".into()),
                GUEST_MEMORY_MODE => modes.map(Into::into),
                _ => None,
            })
        };
        let unset = settings(None)?;
        assert_eq!(unset.guest_modes, [GuestMemoryMode::Enforce]);
        assert_eq!(unset.guest_mode, GuestMemoryMode::Enforce);
        let both = settings(Some("enforce, growth"))?;
        assert_eq!(
            both.guest_modes,
            [GuestMemoryMode::Enforce, GuestMemoryMode::Growth]
        );
        assert_eq!(both.guest_mode, GuestMemoryMode::Enforce);
        assert!(settings(Some("enforce,sideways")).is_err());
        Ok(())
    }

    #[test]
    fn memory_modes_publish_distinct_series() {
        assert_eq!(report_label("warm", None, GuestMemoryMode::Enforce), "warm");
        assert_eq!(
            report_label("warm", Some(2), GuestMemoryMode::Count),
            "warm@2cpu-count"
        );
        assert_eq!(
            report_label("warm", None, GuestMemoryMode::Growth),
            "warm-growth"
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
