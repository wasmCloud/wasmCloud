//! The measurement loop: start workloads in steps, prove them live, measure,
//! and stop at the first sign the host is full.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::{StreamExt as _, stream};
use serde::Serialize;

use crate::api::{self, HostClient, RpcError, WorkloadState};
use crate::host::{ChildGuard, log_tail};
use crate::memory::{MemorySource, Sampler, Watch};
use crate::registry::RegistryCounts;
use crate::stats::{millis, percentile, spread};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flavour {
    Warm,
    ScaleToZero,
}

impl Flavour {
    pub fn name(self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::ScaleToZero => "scale-to-zero",
        }
    }

    fn pool_size(self) -> i32 {
        match self {
            Self::Warm => 1,
            Self::ScaleToZero => 0,
        }
    }
}

/// What one flavour runs with, resolved from the flags.
#[derive(Debug, Clone, Serialize)]
pub struct Settings {
    pub memory_limit_bytes: u64,
    pub max_components: Option<usize>,
    pub overshoot: f64,
    pub step: usize,
    pub start_concurrency: usize,
    pub start_timeout_secs: u64,
    pub start_retries: u32,
    pub sample: usize,
    pub request_concurrency: usize,
    pub request_timeout_secs: u64,
    pub spin_down_wait_ms: u64,
    pub core_instances: u32,
    pub core_instances_per_workload: Option<u32>,
    pub min_components: usize,
}

/// How the host refused a start, by its message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefusalClass {
    /// The pooling allocator is out of slots (core instances, memories,
    /// tables, stacks).
    PoolExhausted,
    /// A memory budget or limit refused it.
    MemoryLimit,
    Other,
}

impl RefusalClass {
    fn describe(self) -> &'static str {
        match self {
            Self::PoolExhausted => "pool exhausted",
            Self::MemoryLimit => "memory limit",
            Self::Other => "other",
        }
    }
}

pub fn classify_refusal(message: &str) -> RefusalClass {
    let lower = message.to_ascii_lowercase();
    if lower.contains("maximum concurrent limit")
        || lower.contains("pooling allocator")
        || lower.contains("core instances")
    {
        RefusalClass::PoolExhausted
    } else if lower.contains("max-process-memory")
        || lower.contains("process memory")
        || lower.contains("guest memory")
        || lower.contains("memory limit")
        || lower.contains("memory budget")
        || lower.contains("out of memory")
    {
        RefusalClass::MemoryLimit
    } else {
        RefusalClass::Other
    }
}

/// N out of wasmtime's "maximum concurrent limit of N for core instances".
pub fn core_instance_limit(message: &str) -> Option<u64> {
    let (_, rest) = message.split_once("maximum concurrent limit of ")?;
    let (number, kind) = rest.split_once(' ')?;
    kind.starts_with("for core instances")
        .then(|| number.parse().ok())
        .flatten()
}

/// Why a flavour stopped.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum Stop {
    /// The host answered a start with WORKLOAD_STATE_ERROR.
    StartRefused {
        class: RefusalClass,
        #[serde(serialize_with = "crate::report::serialize_diagnostic")]
        message: String,
    },
    /// The host's footprint reached the memory limit.
    MemoryLimit {
        footprint_bytes: u64,
        limit_bytes: u64,
    },
    /// A warm-up failed, or more than half the sampled requests failed.
    Requests {
        stage: &'static str,
        failed: usize,
        total: usize,
        #[serde(serialize_with = "crate::report::serialize_optional_diagnostic")]
        example: Option<String>,
    },
    /// The host process ended.
    HostExited {
        status: String,
        #[serde(serialize_with = "crate::report::serialize_diagnostic")]
        log_tail: String,
    },
    /// Starts got no answer even after retries.
    StartsUnanswered {
        count: usize,
        #[serde(serialize_with = "crate::report::serialize_diagnostic")]
        example: String,
    },
    /// Starts failed for reasons other than a refusal.
    StartsFailed {
        count: usize,
        #[serde(serialize_with = "crate::report::serialize_diagnostic")]
        example: String,
    },
    /// The component cap was reached.
    Cap { cap: usize },
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StartRefused { class, .. } => {
                write!(
                    f,
                    "start refused ({}): {}",
                    class.describe(),
                    crate::report::OMITTED_DIAGNOSTIC
                )
            }
            Self::MemoryLimit {
                footprint_bytes,
                limit_bytes,
            } => write!(
                f,
                "memory limit: footprint {} >= {}",
                crate::size::render(*footprint_bytes),
                crate::size::render(*limit_bytes)
            ),
            Self::Requests {
                stage,
                failed,
                total,
                example,
            } => {
                write!(f, "requests failing: {failed} of {total} {stage} requests")?;
                if example.is_some() {
                    write!(f, " {}", crate::report::OMITTED_DIAGNOSTIC)?;
                }
                Ok(())
            }
            Self::HostExited { status, .. } => write!(f, "host exited: {status}"),
            Self::StartsUnanswered { count, .. } => {
                write!(
                    f,
                    "{count} starts unanswered: {}",
                    crate::report::OMITTED_DIAGNOSTIC
                )
            }
            Self::StartsFailed { count, .. } => write!(
                f,
                "{count} starts failed: {}",
                crate::report::OMITTED_DIAGNOSTIC
            ),
            Self::Cap { cap } => write!(f, "cap of {cap} workloads reached"),
        }
    }
}

/// One measurement, taken after a step of starts.
#[derive(Debug, Clone, Serialize)]
pub struct Step {
    /// Running workloads after this step.
    pub workloads: usize,
    pub attempted: usize,
    pub started: usize,
    pub elapsed_secs: f64,
    pub start_secs: f64,
    pub starts_per_sec: f64,
    pub start_p50_ms: Option<f64>,
    pub start_p99_ms: Option<f64>,
    pub start_refused: usize,
    /// Start replies that never came, each followed by a status query.
    pub start_timeouts: u32,
    /// Starts that found nobody listening.
    pub start_no_responders: u32,
    pub start_retries: u32,
    pub start_unanswered: usize,
    pub start_failed: usize,
    /// Warm: the first request to each new workload.
    pub warmup_ok: usize,
    pub warmup_failed: usize,
    pub warmup_p50_ms: Option<f64>,
    pub warmup_p99_ms: Option<f64>,
    pub footprint_bytes: Option<u64>,
    pub peak_footprint_bytes: Option<u64>,
    pub bytes_per_workload: Option<u64>,
    pub heartbeat_workloads: Option<u64>,
    pub heartbeat_components: Option<u64>,
    pub requests_ok: usize,
    pub requests_failed: usize,
    pub requests_per_sec: f64,
    pub request_p50_ms: Option<f64>,
    pub request_p99_ms: Option<f64>,
    /// Scale-to-zero: footprint the sample added, and how long it took to
    /// drop back. `None` when it did not settle within --spin-down-wait.
    pub sample_growth_bytes: Option<u64>,
    pub spin_down_ms: Option<f64>,
    pub theoretical_limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostInfo {
    pub binary: Option<String>,
    #[serde(serialize_with = "crate::report::serialize_args")]
    pub args: Vec<String>,
    pub unsupported_flags: Vec<String>,
    pub id: String,
    pub version: String,
    pub hostname: String,
    #[serde(serialize_with = "crate::report::serialize_http_address")]
    pub http: String,
    pub pid: Option<u32>,
    pub log: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub achieved: usize,
    pub served: usize,
    pub theoretical_limit: Option<u64>,
    pub bytes_per_workload: Option<u64>,
    pub peak_footprint_bytes: Option<u64>,
    pub starts_per_sec: f64,
    pub start_p50_ms: Option<f64>,
    pub start_p99_ms: Option<f64>,
    pub warmup_p50_ms: Option<f64>,
    pub warmup_p99_ms: Option<f64>,
    pub request_p50_ms: Option<f64>,
    pub request_p99_ms: Option<f64>,
    pub start_timeouts: u32,
    pub start_no_responders: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlavourReport {
    pub flavour: Flavour,
    pub host: HostInfo,
    pub memory_source: MemorySource,
    pub baseline_footprint_bytes: Option<u64>,
    /// Pool slots the host was given, or reported in a refusal.
    pub core_instances: Option<u64>,
    pub core_instances_per_workload: Option<u64>,
    pub steps: Vec<Step>,
    pub stop: Stop,
    pub summary: Summary,
    pub registry_requests: Option<RegistryCounts>,
}

/// A host ready to bench: its API, where its HTTP listens, its process.
pub struct Target {
    pub client: HostClient,
    pub info: HostInfo,
    pub sampler: Arc<Sampler>,
    pub guard: Option<Arc<Mutex<ChildGuard>>>,
    pub log: Option<PathBuf>,
    /// Core instances the host was told it has, when the bench told it.
    pub core_instances: Option<u32>,
    /// The bench owns the host and kills it; otherwise it stops what it
    /// started.
    pub owned: bool,
}

impl Target {
    /// Kill an owned host now rather than whenever the last handle drops.
    pub fn shutdown(&self) {
        if let Some(guard) = &self.guard
            && let Ok(mut guard) = guard.lock()
        {
            guard.kill();
        }
    }

    fn exited(&self) -> Option<Stop> {
        let guard = self.guard.as_ref()?;
        let status = guard.lock().ok()?.exited()?;
        Some(Stop::HostExited {
            status: status.to_string(),
            log_tail: self
                .log
                .as_deref()
                .map(|l| log_tail(l, 30))
                .unwrap_or_default(),
        })
    }
}

enum Started {
    Running,
    Refused(String),
    Unanswered(String),
    Failed(String),
    Skipped,
}

struct StartOutcome {
    result: Started,
    latency: Duration,
    timeouts: u32,
    no_responders: u32,
    retries: u32,
}

struct Starter<'a> {
    client: &'a HostClient,
    image: &'a str,
    flavour: Flavour,
    run_id: &'a str,
    timeout: Duration,
    retries: u32,
    halt: &'a AtomicBool,
    journal: &'a crate::lifecycle::Journal,
}

fn host_header(run_id: &str, flavour: Flavour, index: usize) -> String {
    format!("w{index}.{}.{run_id}.scale.test", flavour.name())
}

fn workload_id(run_id: &str, flavour: Flavour, index: usize) -> String {
    format!("bench-scale-{run_id}-{}-{index}", flavour.name())
}

impl Starter<'_> {
    async fn start(&self, index: usize) -> StartOutcome {
        let mut outcome = StartOutcome {
            result: Started::Skipped,
            latency: Duration::ZERO,
            timeouts: 0,
            no_responders: 0,
            retries: 0,
        };
        if self.halt.load(Ordering::Relaxed) {
            return outcome;
        }
        let id = workload_id(self.run_id, self.flavour, index);
        let request = api::start_request(
            &id,
            &format!("w{index}"),
            self.image,
            self.flavour.pool_size(),
            &host_header(self.run_id, self.flavour, index),
        );
        self.journal.submitted(&id);
        let began = Instant::now();
        let mut attempt = 0;
        outcome.result = loop {
            let resolved = match self.client.start(&request, self.timeout).await {
                Ok(status) => {
                    self.journal.acknowledged(&id);
                    match status.workload_state {
                        WorkloadState::Running => Some(Started::Running),
                        // A retried start can be refused as a duplicate of the
                        // first, which did start after all.
                        WorkloadState::Error if attempt > 0 => match self.settle(&id).await {
                            Some(Started::Running) => Some(Started::Running),
                            _ => Some(Started::Refused(status.message)),
                        },
                        WorkloadState::Error => Some(Started::Refused(status.message)),
                        WorkloadState::Starting | WorkloadState::Unspecified => {
                            self.settle(&id).await
                        }
                        other => Some(Started::Failed(format!("{other:?}: {}", status.message))),
                    }
                }
                Err(RpcError::Timeout) => {
                    outcome.timeouts += 1;
                    self.settle(&id).await
                }
                Err(RpcError::NoResponders) => {
                    outcome.no_responders += 1;
                    tokio::time::sleep(Duration::from_millis(200 << attempt.min(5))).await;
                    None
                }
                Err(RpcError::Other(e)) => Some(Started::Failed(format!("{e:#}"))),
            };
            if let Some(result) = resolved {
                break result;
            }
            if attempt >= self.retries {
                break Started::Unanswered(format!(
                    "no answer after {} attempts ({} timeouts, {} no responders)",
                    attempt + 1,
                    outcome.timeouts,
                    outcome.no_responders
                ));
            }
            attempt += 1;
            outcome.retries += 1;
        };
        outcome.latency = began.elapsed();
        if matches!(outcome.result, Started::Refused(_) | Started::Unanswered(_)) {
            self.halt.store(true, Ordering::Relaxed);
        }
        outcome
    }

    /// After a start went unanswered: ask its status until it resolves.
    /// `None` means the host never saw it, so it is worth sending again.
    async fn settle(&self, id: &str) -> Option<Started> {
        let deadline = Instant::now() + self.timeout;
        loop {
            match self.client.status(id, Duration::from_secs(5)).await {
                Ok(status) => match status.workload_state {
                    WorkloadState::Running => return Some(Started::Running),
                    WorkloadState::Error => return Some(Started::Refused(status.message)),
                    WorkloadState::NotFound => return None,
                    WorkloadState::Starting | WorkloadState::Unspecified => {}
                    other => {
                        return Some(Started::Failed(format!("{other:?}: {}", status.message)));
                    }
                },
                Err(RpcError::NoResponders) => return None,
                Err(_) => {}
            }
            if Instant::now() > deadline {
                return Some(Started::Unanswered(format!(
                    "{id} still not running after {:?}",
                    self.timeout
                )));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[derive(Default)]
struct Batch {
    started: Vec<usize>,
    latencies: Vec<f64>,
    refused: Vec<String>,
    unanswered: Vec<String>,
    failed: Vec<String>,
    timeouts: u32,
    no_responders: u32,
    retries: u32,
}

async fn start_batch(starter: &Starter<'_>, indices: Range<usize>, concurrency: usize) -> Batch {
    let outcomes: Vec<(usize, StartOutcome)> = stream::iter(indices)
        .map(|index| async move { (index, starter.start(index).await) })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;
    let mut batch = Batch::default();
    for (index, outcome) in outcomes {
        batch.timeouts += outcome.timeouts;
        batch.no_responders += outcome.no_responders;
        batch.retries += outcome.retries;
        match outcome.result {
            Started::Running => {
                batch.started.push(index);
                batch.latencies.push(millis(outcome.latency));
            }
            Started::Refused(message) => batch.refused.push(message),
            Started::Unanswered(message) => batch.unanswered.push(message),
            Started::Failed(message) => batch.failed.push(message),
            Started::Skipped => {}
        }
    }
    batch.started.sort_unstable();
    batch
}

#[derive(Default)]
struct Answers {
    ok: usize,
    failed: usize,
    latencies: Vec<f64>,
    example_error: Option<String>,
}

const ERROR_BODY_LIMIT: usize = 16 * 1024;

/// Measure the whole response without retaining a guest-controlled body.
/// Keep only a bounded diagnostic prefix when the response is unsuccessful.
async fn drain_response(mut response: reqwest::Response) -> Result<(), String> {
    let status = response.status();
    let mut diagnostic = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("{e}"))? {
        if !status.is_success() {
            let retain = chunk
                .len()
                .min(ERROR_BODY_LIMIT.saturating_sub(diagnostic.len()));
            diagnostic.extend_from_slice(&chunk[..retain]);
            truncated |= retain < chunk.len();
        }
    }
    if !status.is_success() {
        return Err(format!(
            "{status}: {}{}",
            String::from_utf8_lossy(&diagnostic),
            if truncated { " [truncated]" } else { "" },
        ));
    }
    Ok(())
}

/// One GET to each workload, from outside the host, routed by Host header.
async fn requests(
    http: &reqwest::Client,
    url: &str,
    run_id: &str,
    flavour: Flavour,
    indices: Vec<usize>,
    concurrency: usize,
) -> Answers {
    let results: Vec<Result<f64, String>> = stream::iter(indices)
        .map(|index| async move {
            let began = Instant::now();
            let response = http
                .get(url)
                .header(reqwest::header::HOST, host_header(run_id, flavour, index))
                .send()
                .await
                .map_err(|e| format!("{e}"))?;
            drain_response(response).await?;
            Ok(millis(began.elapsed()))
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;
    let mut answers = Answers::default();
    for result in results {
        match result {
            Ok(latency) => {
                answers.ok += 1;
                answers.latencies.push(latency);
            }
            Err(e) => {
                answers.failed += 1;
                answers.example_error.get_or_insert(e);
            }
        }
    }
    answers
}

/// The workload count the measurements so far say this host can hold: the
/// lower of what the memory limit and (warm) the pool fit.
pub fn theoretical_limit(
    memory_limit: u64,
    baseline: Option<u64>,
    bytes_per_workload: Option<u64>,
    by_pool: Option<u64>,
) -> Option<u64> {
    let by_memory = match (baseline, bytes_per_workload) {
        (Some(baseline), Some(per)) if per > 0 => Some(memory_limit.saturating_sub(baseline) / per),
        _ => None,
    };
    match (by_memory, by_pool) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The cap a run without --max-components stops at: a little past the
/// theoretical limit, so the run sees how the host fails there.
pub fn dynamic_cap(theoretical: u64, overshoot: f64) -> usize {
    (theoretical as f64 * overshoot.max(1.0)).ceil() as usize
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CapacityEstimate {
    bytes_per_workload: Option<u64>,
    theoretical_limit: Option<u64>,
}

impl CapacityEstimate {
    fn from_step(
        step: &Step,
        flavour: Flavour,
        memory_limit: u64,
        baseline: Option<u64>,
        by_pool: Option<u64>,
    ) -> Option<Self> {
        if step.workloads == 0
            || step.started != step.attempted
            || step.start_refused > 0
            || step.start_unanswered > 0
            || step.start_failed > 0
            || step.requests_ok == 0
            || step.requests_failed > 0
            || step.warmup_failed > 0
            || (flavour == Flavour::Warm && step.warmup_ok != step.started)
        {
            return None;
        }
        let bytes_per_workload = match (step.footprint_bytes, baseline) {
            (Some(now), Some(base)) => Some(now.saturating_sub(base) / step.workloads as u64),
            _ => None,
        };
        let theoretical_limit =
            theoretical_limit(memory_limit, baseline, bytes_per_workload, by_pool);
        (bytes_per_workload.is_some() || theoretical_limit.is_some()).then_some(Self {
            bytes_per_workload,
            theoretical_limit,
        })
    }
}

struct CapacityTracker {
    latest: Option<CapacityEstimate>,
    cap: Option<usize>,
}

impl CapacityTracker {
    fn observe(
        &mut self,
        step: &mut Step,
        flavour: Flavour,
        settings: &Settings,
        baseline: Option<u64>,
        by_pool: Option<u64>,
    ) {
        let Some(estimate) = CapacityEstimate::from_step(
            step,
            flavour,
            settings.memory_limit_bytes,
            baseline,
            by_pool,
        ) else {
            return;
        };
        step.bytes_per_workload = estimate.bytes_per_workload;
        step.theoretical_limit = estimate.theoretical_limit;
        self.latest = Some(estimate);
        if settings.max_components.is_none()
            && let Some(theoretical) = estimate.theoretical_limit
        {
            self.cap = Some(dynamic_cap(theoretical, settings.overshoot));
        }
    }
}

fn checked_footprint(
    footprint: Option<u64>,
    max_components: Option<usize>,
    phase: &str,
) -> Result<Option<u64>> {
    anyhow::ensure!(
        footprint.is_some() || max_components.is_some(),
        "process footprint unavailable {phase}; an uncapped run requires a readable host PID, \
         or set --max-components to bound a run without memory measurements"
    );
    Ok(footprint)
}

/// Wait for the footprint to settle back near `before`; how long that took.
async fn spin_down(sampler: &Sampler, before: u64, wait: Duration) -> Option<f64> {
    let tolerance = (before / 100).max(1 << 20);
    let began = Instant::now();
    while began.elapsed() < wait {
        if sampler.footprint()? <= before + tolerance {
            return Some(millis(began.elapsed()));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

pub struct Run<'a> {
    pub settings: &'a Settings,
    pub image: &'a str,
    pub run_id: &'a str,
    pub on_step: &'a dyn Fn(&Step, bool),
    pub cancel: &'a tokio_util::sync::CancellationToken,
}

pub async fn run_flavour(
    run: &Run<'_>,
    flavour: Flavour,
    target: &Target,
) -> Result<FlavourReport> {
    let journal = crate::lifecycle::Journal::default();
    crate::lifecycle::with_cleanup(
        run.cancel,
        &target.client,
        &journal,
        !target.owned,
        run.settings.start_concurrency,
        measure_flavour(run, flavour, target, &journal),
    )
    .await
}

async fn measure_flavour(
    run: &Run<'_>,
    flavour: Flavour,
    target: &Target,
    journal: &crate::lifecycle::Journal,
) -> Result<FlavourReport> {
    let settings = run.settings;
    let http = reqwest::Client::builder()
        .pool_max_idle_per_host(settings.request_concurrency)
        .timeout(Duration::from_secs(settings.request_timeout_secs))
        .build()?;
    let url = format!("http://{}/", target.info.http);

    // Let the host's startup allocations land before taking the baseline.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let baseline = checked_footprint(
        target.sampler.footprint(),
        settings.max_components,
        "at baseline",
    )?;
    let halt = Arc::new(AtomicBool::new(false));
    let watch = Watch::spawn(
        Arc::clone(&target.sampler),
        settings.memory_limit_bytes,
        Arc::clone(&halt),
    );
    let exit_watch = target.guard.as_ref().map(|guard| {
        let guard = Arc::clone(guard);
        let halt = Arc::clone(&halt);
        tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if guard.lock().ok().and_then(|mut g| g.exited()).is_some() {
                    halt.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }))
    });

    let starter = Starter {
        client: &target.client,
        image: run.image,
        flavour,
        run_id: run.run_id,
        timeout: Duration::from_secs(settings.start_timeout_secs),
        retries: settings.start_retries,
        halt: &halt,
        journal,
    };
    let mut pool_total = target.core_instances.map(u64::from);
    let per_workload = settings.core_instances_per_workload.map(u64::from);
    let pool_bound = |total: Option<u64>, per: Option<u64>| match (flavour, total, per) {
        (Flavour::Warm, Some(total), Some(per)) if per > 0 => Some(total / per),
        _ => None,
    };

    let began = Instant::now();
    let mut running: Vec<usize> = Vec::new();
    let mut next_index = 0usize;
    let mut steps: Vec<Step> = Vec::new();
    let mut capacity = CapacityTracker {
        latest: None,
        cap: settings.max_components,
    };
    let mut all_start_latencies = Vec::new();
    let mut all_warmup_latencies = Vec::new();
    let mut all_request_latencies = Vec::new();
    let mut total_start_secs = 0.0;

    let stop = loop {
        if let Some(stop) = target.exited() {
            break stop;
        }
        if let Some(cap) = capacity.cap
            && running.len() >= cap
        {
            break Stop::Cap { cap };
        }
        checked_footprint(
            target.sampler.footprint(),
            settings.max_components,
            "before starting another batch",
        )?;
        let room = capacity
            .cap
            .map_or(settings.step, |cap| cap.saturating_sub(running.len()));
        let attempted = settings.step.min(room).max(1);
        let step_began = Instant::now();
        let batch = start_batch(
            &starter,
            next_index..next_index + attempted,
            settings.start_concurrency,
        )
        .await;
        let start_secs = step_began.elapsed().as_secs_f64();
        total_start_secs += start_secs;
        next_index += attempted;
        running.extend_from_slice(&batch.started);
        all_start_latencies.extend_from_slice(&batch.latencies);

        // Warm workloads prove they are live, and park their instance, by
        // answering one request each.
        let warmup = if flavour == Flavour::Warm && !batch.started.is_empty() {
            requests(
                &http,
                &url,
                run.run_id,
                flavour,
                batch.started.clone(),
                settings.request_concurrency,
            )
            .await
        } else {
            Answers::default()
        };
        all_warmup_latencies.extend_from_slice(&warmup.latencies);

        let before_sample = target.sampler.footprint();
        let sample: Vec<usize> = spread(running.len(), settings.sample)
            .into_iter()
            .filter_map(|position| running.get(position).copied())
            .collect();
        let sample_began = Instant::now();
        let answers = requests(
            &http,
            &url,
            run.run_id,
            flavour,
            sample.clone(),
            settings.request_concurrency,
        )
        .await;
        let sample_secs = sample_began.elapsed().as_secs_f64();
        all_request_latencies.extend_from_slice(&answers.latencies);

        let (sample_growth, spin_down_ms) = match (flavour, before_sample) {
            (Flavour::ScaleToZero, Some(before)) => {
                let after = target.sampler.footprint();
                let settled = spin_down(
                    &target.sampler,
                    before,
                    Duration::from_millis(settings.spin_down_wait_ms),
                )
                .await;
                (after.map(|a| a.saturating_sub(before)), settled)
            }
            _ => (None, None),
        };

        let heartbeat = target.client.heartbeat(Duration::from_secs(5)).await.ok();
        let footprint = checked_footprint(
            target.sampler.footprint(),
            settings.max_components,
            "after measuring a batch",
        )?;
        if let Some(limit) = batch.refused.iter().find_map(|m| core_instance_limit(m)) {
            pool_total = Some(limit);
        }

        let mut step = Step {
            workloads: running.len(),
            attempted,
            started: batch.started.len(),
            elapsed_secs: began.elapsed().as_secs_f64(),
            start_secs,
            starts_per_sec: batch.started.len() as f64 / start_secs.max(f64::EPSILON),
            start_p50_ms: percentile(&batch.latencies, 0.50),
            start_p99_ms: percentile(&batch.latencies, 0.99),
            start_refused: batch.refused.len(),
            start_timeouts: batch.timeouts,
            start_no_responders: batch.no_responders,
            start_retries: batch.retries,
            start_unanswered: batch.unanswered.len(),
            start_failed: batch.failed.len(),
            warmup_ok: warmup.ok,
            warmup_failed: warmup.failed,
            warmup_p50_ms: percentile(&warmup.latencies, 0.50),
            warmup_p99_ms: percentile(&warmup.latencies, 0.99),
            footprint_bytes: footprint,
            peak_footprint_bytes: watch.peak(),
            bytes_per_workload: None,
            heartbeat_workloads: heartbeat.as_ref().map(|h| h.workload_count),
            heartbeat_components: heartbeat.as_ref().map(|h| h.component_count),
            requests_ok: answers.ok,
            requests_failed: answers.failed,
            requests_per_sec: answers.ok as f64 / sample_secs.max(f64::EPSILON),
            request_p50_ms: percentile(&answers.latencies, 0.50),
            request_p99_ms: percentile(&answers.latencies, 0.99),
            sample_growth_bytes: sample_growth,
            spin_down_ms,
            theoretical_limit: None,
        };
        capacity.observe(
            &mut step,
            flavour,
            settings,
            baseline,
            pool_bound(pool_total, per_workload),
        );
        (run.on_step)(&step, steps.is_empty());
        steps.push(step);

        if let Some(stop) = target.exited() {
            break stop;
        }
        let over = footprint
            .filter(|f| *f >= settings.memory_limit_bytes)
            .or_else(|| {
                watch
                    .over
                    .load(Ordering::Relaxed)
                    .then(|| watch.peak())
                    .flatten()
            });
        if let Some(footprint_bytes) = over {
            break Stop::MemoryLimit {
                footprint_bytes,
                limit_bytes: settings.memory_limit_bytes,
            };
        }
        if let Some(message) = batch.refused.first() {
            break Stop::StartRefused {
                class: classify_refusal(message),
                message: message.clone(),
            };
        }
        if let Some(example) = batch.unanswered.first() {
            break Stop::StartsUnanswered {
                count: batch.unanswered.len(),
                example: example.clone(),
            };
        }
        if warmup.failed > 0 {
            break Stop::Requests {
                stage: "warm-up",
                failed: warmup.failed,
                total: warmup.ok + warmup.failed,
                example: warmup.example_error,
            };
        }
        if answers.failed * 2 > sample.len() {
            break Stop::Requests {
                stage: "sampled",
                failed: answers.failed,
                total: sample.len(),
                example: answers.example_error,
            };
        }
        if let Some(example) = batch.failed.first()
            && batch.failed.len() * 2 > attempted
        {
            break Stop::StartsFailed {
                count: batch.failed.len(),
                example: example.clone(),
            };
        }
    };
    if let Some(task) = exit_watch {
        task.abort();
    }
    drop(watch);

    let mut serving = bench_tools::density::ServingEvidence::default();
    let served = steps
        .iter()
        .filter(|s| serving.observe(s.requests_ok, s.requests_failed, s.warmup_failed))
        .map(|s| s.workloads)
        .max()
        .unwrap_or(0);
    let summary = Summary {
        achieved: running.len(),
        served,
        theoretical_limit: capacity.latest.and_then(|s| s.theoretical_limit),
        bytes_per_workload: capacity.latest.and_then(|s| s.bytes_per_workload),
        peak_footprint_bytes: steps.iter().filter_map(|s| s.peak_footprint_bytes).max(),
        starts_per_sec: running.len() as f64 / total_start_secs.max(f64::EPSILON),
        start_p50_ms: percentile(&all_start_latencies, 0.50),
        start_p99_ms: percentile(&all_start_latencies, 0.99),
        warmup_p50_ms: percentile(&all_warmup_latencies, 0.50),
        warmup_p99_ms: percentile(&all_warmup_latencies, 0.99),
        request_p50_ms: percentile(&all_request_latencies, 0.50),
        request_p99_ms: percentile(&all_request_latencies, 0.99),
        start_timeouts: steps.iter().map(|s| s.start_timeouts).sum(),
        start_no_responders: steps.iter().map(|s| s.start_no_responders).sum(),
    };
    Ok(FlavourReport {
        flavour,
        host: target.info.clone(),
        memory_source: target.sampler.source(),
        baseline_footprint_bytes: baseline,
        core_instances: pool_total,
        core_instances_per_workload: per_workload,
        steps,
        stop,
        summary,
        registry_requests: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn response_bodies_are_drained_with_bounded_error_examples() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        for (status, complete, success) in [
            ("200 OK", true, true),
            ("500 Internal Server Error", true, false),
            ("200 OK", false, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    header.push(socket.read_u8().await.unwrap());
                    assert!(header.len() <= 4096);
                }
                assert!(
                    String::from_utf8(header)
                        .unwrap()
                        .to_ascii_lowercase()
                        .lines()
                        .any(|line| line == "host: w0.warm.response-test.scale.test")
                );
                let head = format!(
                    "HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                );
                socket.write_all(head.as_bytes()).await.unwrap();
                // A body much larger than the retained error prefix, emitted
                // incrementally so the server need not buffer it either.
                let chunk = vec![b'x'; 4096];
                for _ in 0..128 {
                    socket.write_all(b"1000\r\n").await.unwrap();
                    socket.write_all(&chunk).await.unwrap();
                    socket.write_all(b"\r\n").await.unwrap();
                }
                if complete {
                    socket.write_all(b"0\r\n\r\n").await.unwrap();
                }
            });
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let answers = requests(&client, &url, "response-test", Flavour::Warm, vec![0], 1).await;
            server.await.unwrap();
            assert_eq!(answers.ok, usize::from(success));
            assert_eq!(answers.failed, usize::from(!success));
            if status.starts_with("500") {
                let example = answers.example_error.unwrap();
                assert!(example.len() <= ERROR_BODY_LIMIT + 100);
                assert!(example.starts_with("500 Internal Server Error: "));
                assert!(example.ends_with(" [truncated]"));
            }
        }
    }

    #[test]
    fn classifies_refusals() {
        assert_eq!(
            classify_refusal(
                "failed to instantiate: maximum concurrent limit of 1000 for core instances reached"
            ),
            RefusalClass::PoolExhausted
        );
        assert_eq!(
            classify_refusal("maximum concurrent limit of 500 for memories reached"),
            RefusalClass::PoolExhausted
        );
        assert_eq!(
            classify_refusal("process memory is close to or past max-process-memory"),
            RefusalClass::MemoryLimit
        );
        assert_eq!(
            classify_refusal("guest memory budget exhausted"),
            RefusalClass::MemoryLimit
        );
        assert_eq!(
            classify_refusal("failed to pull component from 127.0.0.1:1/x:y"),
            RefusalClass::Other
        );
    }

    #[test]
    fn reads_the_core_instance_limit() {
        assert_eq!(
            core_instance_limit(
                "instantiate: maximum concurrent limit of 13106 for core instances reached"
            ),
            Some(13_106)
        );
        assert_eq!(
            core_instance_limit("maximum concurrent limit of 10 for memories reached"),
            None
        );
        assert_eq!(core_instance_limit("something else"), None);
    }

    #[test]
    fn theoretical_is_the_tighter_bound() {
        let mib = 1 << 20;
        assert_eq!(
            theoretical_limit(1024 * mib, Some(24 * mib), Some(mib), None),
            Some(1000)
        );
        assert_eq!(
            theoretical_limit(1024 * mib, Some(24 * mib), Some(mib), Some(400)),
            Some(400)
        );
        assert_eq!(
            theoretical_limit(1024 * mib, None, None, Some(400)),
            Some(400)
        );
        assert_eq!(
            theoretical_limit(1024 * mib, Some(24 * mib), Some(0), None),
            None
        );
        assert_eq!(
            theoretical_limit(mib, Some(2 * mib), Some(1), None),
            Some(0)
        );
        assert_eq!(dynamic_cap(1000, 1.1), 1100);
        assert_eq!(dynamic_cap(1000, 0.5), 1000);
    }

    #[test]
    fn failed_terminal_steps_preserve_the_verified_estimate_and_cap() {
        let settings = Settings {
            memory_limit_bytes: 2100,
            max_components: None,
            overshoot: 1.1,
            step: 250,
            start_concurrency: 10,
            start_timeout_secs: 30,
            start_retries: 0,
            sample: 200,
            request_concurrency: 10,
            request_timeout_secs: 5,
            spin_down_wait_ms: 0,
            core_instances: 10_000,
            core_instances_per_workload: None,
            min_components: 0,
        };
        let measured_step = || Step {
            workloads: 250,
            attempted: 250,
            started: 250,
            elapsed_secs: 1.0,
            start_secs: 1.0,
            starts_per_sec: 250.0,
            start_p50_ms: None,
            start_p99_ms: None,
            start_refused: 0,
            start_timeouts: 0,
            start_no_responders: 0,
            start_retries: 0,
            start_unanswered: 0,
            start_failed: 0,
            warmup_ok: 250,
            warmup_failed: 0,
            warmup_p50_ms: None,
            warmup_p99_ms: None,
            footprint_bytes: Some(1100),
            peak_footprint_bytes: Some(1100),
            bytes_per_workload: None,
            heartbeat_workloads: None,
            heartbeat_components: None,
            requests_ok: 200,
            requests_failed: 0,
            requests_per_sec: 200.0,
            request_p50_ms: None,
            request_p99_ms: None,
            sample_growth_bytes: None,
            spin_down_ms: None,
            theoretical_limit: None,
        };
        let failures: [fn(&mut Step); 8] = [
            |step| step.warmup_failed = 1,
            |step| step.warmup_ok -= 1,
            |step| step.requests_failed = 1,
            |step| step.requests_ok = 0,
            |step| step.start_refused = 1,
            |step| step.start_unanswered = 1,
            |step| step.start_failed = 1,
            |step| step.started -= 1,
        ];
        for fail in failures {
            let mut capacity = CapacityTracker {
                latest: None,
                cap: None,
            };
            let mut first = measured_step();
            capacity.observe(&mut first, Flavour::Warm, &settings, Some(100), None);
            let verified = CapacityEstimate {
                bytes_per_workload: Some(4),
                theoretical_limit: Some(500),
            };
            assert_eq!(capacity.latest, Some(verified));
            assert_eq!(capacity.cap, Some(550));

            // The new batch reports started workloads without paying all of
            // their expected memory costs. Its unverified slope would double
            // both the predicted capacity and the automatic stopping cap.
            let mut terminal = measured_step();
            terminal.workloads = 500;
            fail(&mut terminal);
            capacity.observe(&mut terminal, Flavour::Warm, &settings, Some(100), None);
            assert_eq!(terminal.bytes_per_workload, None);
            assert_eq!(terminal.theoretical_limit, None);
            assert_eq!(capacity.latest, Some(verified));
            assert_eq!(capacity.cap, Some(550));
        }
    }

    #[test]
    fn uncapped_runs_require_initial_and_subsequent_memory_readings() {
        for phase in [
            "at baseline",
            "before starting another batch",
            "after measuring a batch",
        ] {
            let error = checked_footprint(None, None, phase).unwrap_err();
            assert!(error.to_string().contains(phase));
            assert!(error.to_string().contains("--max-components"));
        }

        // Losing a PID after a valid baseline must stop an uncapped run,
        // even if earlier samples could have supplied an automatic cap.
        assert_eq!(
            checked_footprint(Some(1024), None, "at baseline").unwrap(),
            Some(1024)
        );
        assert!(checked_footprint(None, None, "before starting another batch").is_err());
        // An explicit workload count bounds an intentionally unmeasured run.
        assert_eq!(
            checked_footprint(None, Some(10), "at baseline").unwrap(),
            None
        );
        assert_eq!(
            checked_footprint(None, Some(10), "after measuring a batch").unwrap(),
            None
        );
        // Zero is a measurement, not a missing-value sentinel.
        assert_eq!(
            checked_footprint(Some(0), None, "at baseline").unwrap(),
            Some(0)
        );
    }

    #[test]
    fn stop_reasons_serialize_with_a_tag() {
        let stop = Stop::StartRefused {
            class: RefusalClass::PoolExhausted,
            message: "maximum concurrent limit of 5 for core instances reached".into(),
        };
        let json = serde_json::to_value(&stop).unwrap();
        assert_eq!(json["reason"], "start-refused");
        assert_eq!(json["class"], "pool-exhausted");
        assert!(
            stop.to_string()
                .starts_with("start refused (pool exhausted)")
        );
        let json = serde_json::to_value(Stop::Cap { cap: 3 }).unwrap();
        assert_eq!(json, serde_json::json!({"reason": "cap", "cap": 3}));
    }

    #[test]
    fn names_are_rfc_1123() {
        for flavour in [Flavour::Warm, Flavour::ScaleToZero] {
            let name = host_header("0123456789abcdef0123456789abcdef", flavour, usize::MAX);
            assert!(name.len() <= 253);
            for label in name.split('.') {
                assert!(!label.is_empty() && label.len() <= 63);
                assert!(
                    label
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                );
                assert!(!label.starts_with('-') && !label.ends_with('-'));
            }
        }
    }

    #[test]
    fn simultaneous_runs_have_distinct_workloads_and_http_routes() {
        let first = "0123456789abcdef0123456789abcdef";
        let second = "fedcba9876543210fedcba9876543210";
        for flavour in [Flavour::Warm, Flavour::ScaleToZero] {
            assert_ne!(
                workload_id(first, flavour, 0),
                workload_id(second, flavour, 0)
            );
            assert_ne!(
                host_header(first, flavour, 0),
                host_header(second, flavour, 0)
            );
        }
    }
}
