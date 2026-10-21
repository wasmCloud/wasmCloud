//! The measurement loop: start workloads in steps, prove them live, measure,
//! and stop at the first sign the host is full.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use bench_tools::density::{
    Flavour, Report, ServingEvidence, Step, memory_estimate, percentile, print_step, spread,
};
use futures::{StreamExt as _, stream};
use wash_runtime::engine::host_memory::render_bytes;
use wash_runtime::washlet::types::v2::WorkloadState;

use crate::api::{self, HostClient, RpcError};
use crate::cli::Args;
use crate::lifecycle::Journal;
use crate::memory::{Watch, footprint};

/// How the host refused a start, by its message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalClass {
    /// The pooling allocator is out of slots (core instances, memories,
    /// tables, stacks).
    PoolExhausted,
    /// A memory budget or limit refused it.
    MemoryLimit,
    Other,
}

pub fn classify_refusal(message: &str) -> RefusalClass {
    let lower = message.to_ascii_lowercase();
    if [
        "maximum concurrent limit",
        "pooling allocator",
        "core instances",
    ]
    .iter()
    .any(|m| lower.contains(m))
    {
        RefusalClass::PoolExhausted
    } else if [
        "max-process-memory",
        "process memory",
        "guest memory",
        "memory limit",
        "memory budget",
        "out of memory",
    ]
    .iter()
    .any(|m| lower.contains(m))
    {
        RefusalClass::MemoryLimit
    } else {
        RefusalClass::Other
    }
}

/// Why a flavour stopped. Host messages and response bodies are arbitrary
/// text that may echo credentials, so [`Display`](std::fmt::Display), which
/// goes into the shareable report, leaves them out; [`Stop::diagnostic`]
/// keeps them for stderr.
#[derive(Debug, Clone)]
pub enum Stop {
    /// The host answered a start with WORKLOAD_STATE_ERROR.
    StartRefused {
        class: RefusalClass,
        message: String,
    },
    /// The host's footprint reached the memory limit.
    MemoryLimit { footprint: u64, limit: u64 },
    /// A warm-up failed, or more than half the sampled requests failed.
    Requests {
        stage: &'static str,
        failed: usize,
        total: usize,
        example: Option<String>,
    },
    /// Starts got no answer even after retries.
    StartsUnanswered { count: usize, example: String },
    /// Most of a step's starts failed for reasons other than a refusal.
    StartsFailed { count: usize, example: String },
    /// The component cap was reached.
    Cap { cap: usize },
}

impl Stop {
    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::StartRefused { message: d, .. }
            | Self::StartsUnanswered { example: d, .. }
            | Self::StartsFailed { example: d, .. } => Some(d),
            Self::Requests { example, .. } => example.as_deref(),
            Self::MemoryLimit { .. } | Self::Cap { .. } => None,
        }
    }
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StartRefused { class, .. } => write!(f, "start refused ({class:?})"),
            Self::MemoryLimit { footprint, limit } => write!(
                f,
                "memory limit: footprint {} >= {}",
                render_bytes(*footprint),
                render_bytes(*limit)
            ),
            Self::Requests {
                stage,
                failed,
                total,
                ..
            } => write!(f, "requests failing: {failed} of {total} {stage} requests"),
            Self::StartsUnanswered { count, .. } => write!(f, "{count} starts unanswered"),
            Self::StartsFailed { count, .. } => write!(f, "{count} starts failed"),
            Self::Cap { cap } => write!(f, "cap of {cap} workloads reached"),
        }
    }
}

/// What every flavour of one run shares.
pub struct Run<'a> {
    pub args: &'a Args,
    pub memory_limit: u64,
    pub run_id: &'a str,
    pub client: &'a HostClient,
    /// What the report says ran: the image, and the host.
    pub subject: String,
    pub cancel: &'a tokio_util::sync::CancellationToken,
}

enum Started {
    Running,
    Refused(String),
    Unanswered(String),
    Failed(String),
    Skipped,
}

struct Starter<'a> {
    client: &'a HostClient,
    image: &'a str,
    flavour: Flavour,
    run_id: &'a str,
    timeout: Duration,
    retries: u32,
    halt: &'a AtomicBool,
    journal: &'a Journal,
}

/// An RFC 1123 hostname per workload, unique to this run, so concurrent runs
/// on one host do not route to each other's workloads.
fn host_header(run_id: &str, flavour: Flavour, index: usize) -> String {
    format!("w{index}.{}.{run_id}.scale.test", flavour.name())
}

fn workload_id(run_id: &str, flavour: Flavour, index: usize) -> String {
    format!("bench-scale-{run_id}-{}-{index}", flavour.name())
}

impl Starter<'_> {
    /// The result and its latency.
    async fn start(&self, index: usize) -> (Started, Duration) {
        if self.halt.load(Ordering::Relaxed) {
            return (Started::Skipped, Duration::ZERO);
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
        let (mut timeouts, mut no_responders) = (0, 0);
        let mut attempt = 0;
        let result = loop {
            let resolved = match self.client.start(&request, self.timeout).await {
                Ok(status) => {
                    self.journal.acknowledged(&id);
                    match status.state {
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
                    timeouts += 1;
                    self.settle(&id).await
                }
                Err(RpcError::NoResponders) => {
                    no_responders += 1;
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
                    "no answer after {} attempts ({timeouts} timeouts, {no_responders} no responders)",
                    attempt + 1,
                ));
            }
            attempt += 1;
        };
        if matches!(result, Started::Refused(_) | Started::Unanswered(_)) {
            self.halt.store(true, Ordering::Relaxed);
        }
        (result, began.elapsed())
    }

    /// After a start went unanswered: ask its status until it resolves.
    /// `None` means the host never saw it, so it is worth sending again.
    async fn settle(&self, id: &str) -> Option<Started> {
        let deadline = Instant::now() + self.timeout;
        loop {
            match self.client.status(id, Duration::from_secs(5)).await {
                Ok(status) => match status.state {
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
}

async fn start_batch(starter: &Starter<'_>, indices: Range<usize>, concurrency: usize) -> Batch {
    let outcomes: Vec<(usize, (Started, Duration))> = stream::iter(indices)
        .map(|index| async move { (index, starter.start(index).await) })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;
    let mut batch = Batch::default();
    for (index, (result, latency)) in outcomes {
        match result {
            Started::Running => {
                batch.started.push(index);
                batch.latencies.push(latency.as_secs_f64() * 1000.0);
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
            diagnostic.extend_from_slice(chunk.get(..retain).unwrap_or_default());
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
            Ok(began.elapsed().as_secs_f64() * 1000.0)
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

/// The cap a run without --max-components stops at: a little past the
/// theoretical limit, so the run sees how the host fails there.
pub fn dynamic_cap(theoretical: u64, overshoot: f64) -> usize {
    (theoretical as f64 * overshoot.max(1.0)).ceil() as usize
}

fn checked_footprint(
    footprint: Option<u64>,
    max_components: Option<usize>,
    phase: &str,
) -> Result<Option<u64>> {
    anyhow::ensure!(
        footprint.is_some() || max_components.is_some(),
        "host footprint unavailable {phase}; an uncapped run requires a readable --host-pid, \
         or set --max-components to bound a run without memory measurements"
    );
    Ok(footprint)
}

/// Wait for the footprint to settle back near `before`; how long that took.
async fn spin_down(pid: Option<u32>, before: u64, wait: Duration) -> Option<f64> {
    let tolerance = (before / 100).max(1 << 20);
    let began = Instant::now();
    while began.elapsed() < wait {
        if footprint(pid)? <= before + tolerance {
            return Some(began.elapsed().as_secs_f64() * 1000.0);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

/// Run one flavour, then stop every workload it started. A cleanup failure
/// comes back beside the measurement rather than discarding it.
pub async fn run_flavour(
    run: &Run<'_>,
    flavour: Flavour,
) -> Result<((Report, Stop), Option<String>)> {
    let journal = Journal::default();
    crate::lifecycle::with_cleanup(
        run.cancel,
        run.client,
        &journal,
        run.args.start_concurrency,
        measure_flavour(run, flavour, &journal),
    )
    .await
}

async fn measure_flavour(
    run: &Run<'_>,
    flavour: Flavour,
    journal: &Journal,
) -> Result<(Report, Stop)> {
    let args = run.args;
    let pid = args.host_pid;
    let http = reqwest::Client::builder()
        .pool_max_idle_per_host(args.request_concurrency)
        .timeout(Duration::from_secs(args.request_timeout))
        .build()?;
    let url = format!("http://{}/", args.host_http);

    let baseline = checked_footprint(footprint(pid), args.max_components, "at baseline")?;
    let halt = Arc::new(AtomicBool::new(false));
    let watch = Watch::spawn(
        move || footprint(pid),
        run.memory_limit,
        Arc::clone(&halt),
        args.max_components.is_none(),
    );
    let starter = Starter {
        client: run.client,
        image: &args.image,
        flavour,
        run_id: run.run_id,
        timeout: Duration::from_secs(args.start_timeout),
        retries: args.start_retries,
        halt: &halt,
        journal,
    };

    let mut running: Vec<usize> = Vec::new();
    let mut next_index = 0;
    let mut steps: Vec<Step> = Vec::new();
    let mut serving = ServingEvidence::default();
    let mut estimate = None;
    let mut cap = args.max_components;

    let stop = loop {
        if let Some(cap) = cap
            && running.len() >= cap
        {
            break Stop::Cap { cap };
        }
        watch.check_available()?;
        checked_footprint(
            footprint(pid),
            args.max_components,
            "before starting another batch",
        )?;
        let room = cap.map_or(args.step, |cap| cap.saturating_sub(running.len()));
        let attempted = args.step.min(room).max(1);
        let step_began = Instant::now();
        let indices = next_index..next_index + attempted;
        next_index += attempted;
        let batch = start_batch(&starter, indices, args.start_concurrency).await;
        let start_secs = step_began.elapsed().as_secs_f64();
        running.extend_from_slice(&batch.started);

        // Warm workloads prove they are live, and park their instance, by
        // answering one request each.
        let warmup = if flavour == Flavour::Warm && !batch.started.is_empty() {
            let new = batch.started.clone();
            requests(
                &http,
                &url,
                run.run_id,
                flavour,
                new,
                args.request_concurrency,
            )
            .await
        } else {
            Answers::default()
        };

        let before_sample = footprint(pid);
        let sample: Vec<usize> = spread(running.len(), args.sample)
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
            args.request_concurrency,
        )
        .await;
        let sample_secs = sample_began.elapsed().as_secs_f64();
        let spin_down_ms = match (flavour, before_sample) {
            (Flavour::ScaleToZero, Some(before)) => {
                spin_down(pid, before, Duration::from_millis(args.spin_down_wait)).await
            }
            _ => None,
        };

        watch.check_available()?;
        let footprint_now = checked_footprint(
            footprint(pid),
            args.max_components,
            "after measuring a batch",
        )?;
        let served = serving.observe(answers.ok, answers.failed, warmup.failed);
        let clean = served
            && batch.started.len() == attempted
            && (flavour == Flavour::ScaleToZero || warmup.ok == attempted);
        let workloads = running.len();
        steps.push(Step {
            workloads,
            started: batch.started.len(),
            starts_per_sec: batch.started.len() as f64 / start_secs.max(f64::EPSILON),
            start_p99_ms: percentile(&batch.latencies, 0.99),
            refused: batch.refused.len() as u64,
            footprint_bytes: footprint_now,
            bytes_per_workload: footprint_now
                .zip(baseline)
                .map(|(now, base)| now.saturating_sub(base) / workloads.max(1) as u64),
            requests_ok: answers.ok,
            requests_failed: answers.failed,
            warmup_failed: warmup.failed,
            requests_per_sec: answers.ok as f64 / sample_secs.max(f64::EPSILON),
            request_p50_ms: percentile(&answers.latencies, 0.50),
            request_p99_ms: percentile(&answers.latencies, 0.99),
            spin_down_ms,
            clean,
            ..Default::default()
        });
        if clean
            && let Some(base) = baseline
            && let Some(found) = memory_estimate(&steps, base, run.memory_limit)
        {
            estimate = Some(found);
            if args.max_components.is_none()
                && let (_, Some(fits)) = found
            {
                cap = Some(dynamic_cap(fits, args.overshoot));
            }
        }
        let first = steps.len() == 1;
        if let Some(step) = steps.last_mut() {
            step.theoretical_limit = estimate.and_then(|(_, fits)| fits);
            if args.report {
                print_step(first, step);
            }
        }

        let over = footprint_now
            .filter(|f| *f >= run.memory_limit)
            .or_else(|| {
                watch
                    .over
                    .load(Ordering::Relaxed)
                    .then(|| watch.peak())
                    .flatten()
            });
        if let Some(footprint) = over {
            break Stop::MemoryLimit {
                footprint,
                limit: run.memory_limit,
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

    let report = Report {
        bench: "bench-scale".into(),
        flavour: Some(flavour),
        label: flavour.name().into(),
        subject: run.subject.clone(),
        cores: std::thread::available_parallelism().map_or(1, |n| n.get()),
        memory_limit_bytes: run.memory_limit,
        baseline_footprint_bytes: baseline,
        limits: BTreeMap::from([("memory limit".into(), estimate.and_then(|(_, fits)| fits))]),
        marginal_bytes_per_workload: estimate.map(|(marginal, _)| marginal),
        stop: stop.to_string(),
        steps,
        ..Default::default()
    };
    Ok((report, stop))
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
        for (message, class) in [
            (
                "failed to instantiate: maximum concurrent limit of 1000 for core instances reached",
                RefusalClass::PoolExhausted,
            ),
            (
                "process memory is close to or past max-process-memory",
                RefusalClass::MemoryLimit,
            ),
            ("guest memory budget exhausted", RefusalClass::MemoryLimit),
            (
                "failed to pull component from 127.0.0.1:1/x:y",
                RefusalClass::Other,
            ),
        ] {
            assert_eq!(classify_refusal(message), class, "{message}");
        }
    }

    #[test]
    fn reports_leave_host_diagnostics_out() {
        let stop = Stop::StartRefused {
            class: RefusalClass::Other,
            message: "password=hunter2".into(),
        };
        assert!(!stop.to_string().contains("hunter2"));
        assert_eq!(stop.diagnostic(), Some("password=hunter2"));
    }

    #[test]
    fn uncapped_runs_require_memory_readings() {
        assert!(checked_footprint(None, None, "at baseline").is_err());
        assert_eq!(
            checked_footprint(None, Some(10), "at baseline").unwrap(),
            None
        );
        assert_eq!(
            checked_footprint(Some(1), None, "at baseline").unwrap(),
            Some(1)
        );
    }

    #[test]
    fn names_are_rfc_1123_and_distinct_per_run() {
        let (first, second) = (
            "0123456789abcdef0123456789abcdef",
            "fedcba9876543210fedcba9876543210",
        );
        for flavour in Flavour::ALL {
            let name = host_header(first, flavour, usize::MAX);
            assert!(name.len() <= 253);
            for label in name.split('.') {
                assert!(!label.is_empty() && label.len() <= 63);
                assert!(
                    label
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                );
            }
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
