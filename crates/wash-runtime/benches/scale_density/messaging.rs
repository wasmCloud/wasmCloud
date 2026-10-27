//! The messaging flavours: bursts of slow deliveries on the in-memory backend,
//! grown each step until admission sheds or the pool refuses.
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use bench_tools::density::{Report, Step, print_step};
use tracing_subscriber::Layer;
use tracing_subscriber::registry::LookupSpan;
use wash_runtime::engine::Engine;
use wash_runtime::engine::host_memory::render_bytes;
use wash_runtime::engine::process_memory::process_footprint;
use wash_runtime::host::http::{DynamicRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::wasmcloud_messaging::{InMemoryMessaging, MessagingLimits};
use wash_runtime::types::{
    Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState, WorkloadStopRequest,
};
use wash_runtime::wit::WitInterface;

use super::{
    CpuLimit, GUARD_SLACK_PERCENT, Load, Refusals, Settings, Stop, fixture_bytes, fixture_digest,
    host_cpu, host_header, http_interface, label, measured_footprint, memory_stop,
};

const FIXTURE: &str = "messaging_sleeper_p3";
const SUBJECT: &str = "scale.msg";
/// The span the in-memory backend opens for each admitted delivery.
const DELIVERY_SPAN: &str = "incoming_wasmcloud_message_memory";
const MESSAGING_TARGET: &str = "wash_runtime::plugin::wasmcloud_messaging";
/// How often a burst's peaks are sampled.
const SAMPLE_EVERY: Duration = Duration::from_millis(10);

/// Deliveries as the messaging plugin reports them: a delivery is in flight
/// while the span it opens on admission is open, and one event says how it
/// ended.
#[derive(Default)]
pub(super) struct Deliveries {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    completed: AtomicUsize,
    shed: AtomicUsize,
    failed: AtomicUsize,
}

static DELIVERIES: Deliveries = Deliveries {
    in_flight: AtomicUsize::new(0),
    peak: AtomicUsize::new(0),
    completed: AtomicUsize::new(0),
    shed: AtomicUsize::new(0),
    failed: AtomicUsize::new(0),
};

/// Counts after a burst, against the counts before it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Counts {
    pub(super) completed: usize,
    pub(super) shed: usize,
    pub(super) failed: usize,
}

impl Counts {
    fn since(self, before: Self) -> Self {
        Self {
            completed: self.completed - before.completed,
            shed: self.shed - before.shed,
            failed: self.failed - before.failed,
        }
    }

    fn ended(self) -> usize {
        self.completed + self.shed + self.failed
    }
}

/// The plugin's log messages that say how a delivery ended. A test checks
/// each one still appears in the runtime's source.
const COMPLETED: &str = "message handled successfully";
const SHED: &str = "came free before the deadline;";
const FAILED: [&str; 4] = [
    "message delivery failed",
    "messaging handler returned an error",
    "failed to create store",
    "failed to instantiate",
];

/// How a messaging event ends a delivery, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ending {
    Completed,
    Shed,
    Failed,
}

impl Ending {
    pub(super) fn of(message: &str) -> Option<Self> {
        if message == COMPLETED {
            Some(Self::Completed)
        } else if message.contains(SHED) {
            Some(Self::Shed)
        } else if FAILED.iter().any(|prefix| message.starts_with(prefix)) {
            Some(Self::Failed)
        } else {
            None
        }
    }
}

impl Deliveries {
    fn admitted(&self) {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
    }

    fn left(&self) {
        let _ = self
            .in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
    }

    fn ended(&self, ending: Ending) {
        match ending {
            Ending::Completed => &self.completed,
            Ending::Shed => &self.shed,
            Ending::Failed => &self.failed,
        }
        .fetch_add(1, Ordering::SeqCst);
    }

    fn counts(&self) -> Counts {
        Counts {
            completed: self.completed.load(Ordering::SeqCst),
            shed: self.shed.load(Ordering::SeqCst),
            failed: self.failed.load(Ordering::SeqCst),
        }
    }

    fn idle(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) == 0
    }

    /// Start a burst's peak from what is in flight now.
    fn reset_peak(&self) {
        self.peak
            .store(self.in_flight.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

/// The tracing layer that feeds [`DELIVERIES`].
pub(super) struct DeliveryCounter;

impl DeliveryCounter {
    pub(super) fn layer<S>() -> impl Layer<S>
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        Self.with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.target().starts_with(MESSAGING_TARGET)
        }))
    }
}

impl<S> Layer<S> for DeliveryCounter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if attrs.metadata().name() == DELIVERY_SPAN {
            DELIVERIES.admitted();
        }
    }

    fn on_close(&self, id: tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        if ctx.metadata(&id).is_some_and(|m| m.name() == DELIVERY_SPAN) {
            DELIVERIES.left();
        }
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut message = MessageField::default();
        event.record(&mut message);
        if let Some(ending) = Ending::of(&message.0) {
            DELIVERIES.ended(ending);
        }
    }
}

#[derive(Default)]
struct MessageField(String);

impl tracing::field::Visit for MessageField {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

/// What `MessagingLimits::resolve` sizes the host's ceilings from, so the
/// bench builds them the way `wash host` does: the engine's core-instance
/// total where the runtime takes that, the engine where it reads every pool
/// total.
struct PoolBudget<'a>(&'a Engine);

impl From<PoolBudget<'_>> for Option<u32> {
    // Unused, and deprecated, where `resolve` takes the engine.
    #[allow(deprecated)]
    fn from(budget: PoolBudget<'_>) -> Self {
        budget.0.total_core_instances()
    }
}

impl<'a> From<PoolBudget<'a>> for &'a Engine {
    fn from(budget: PoolBudget<'a>) -> Self {
        budget.0
    }
}

/// The highest readings seen while a burst was in flight.
#[derive(Debug, Default)]
struct Peaks {
    footprint: Option<u64>,
    guest_memory: u64,
    memories: Option<u64>,
    core_instances: Option<u64>,
}

impl Peaks {
    fn sample(&mut self, engine: &Engine) {
        self.footprint = self.footprint.max(process_footprint());
        self.guest_memory = self.guest_memory.max(engine.guest_memory().in_use());
        if let Some(usage) = engine.pool_usage() {
            self.memories = self.memories.max(Some(usage.memories.in_use));
            self.core_instances = self.core_instances.max(Some(usage.core_instances.in_use));
        }
    }
}

pub(super) async fn run(
    flavour: bench_tools::density::Flavour,
    settings: Settings,
    cpu: CpuLimit,
    load: Load,
) -> Result<Report> {
    let wasm = fixture_bytes(FIXTURE)?;
    let digest = fixture_digest(&wasm);
    let hold_ms = settings.msg_hold.as_millis() as u64;
    // Under the hold, so a burst past the ceiling sheds before any slot it
    // waits for comes free.
    let admission_wait_ms = (hold_ms / 2).max(1);

    // The stock pool, so the ceilings are the ones a default host derives.
    let engine = Engine::builder()
        .with_max_process_memory_bytes(settings.memory_limit)
        .build()?;
    anyhow::ensure!(
        engine.process_memory().is_some(),
        "the engine was built without a process memory guard"
    );
    let limits = MessagingLimits::resolve(None, None, PoolBudget(&engine).into());
    let (host_total, per_component) = (limits.host_total(), limits.per_component_default());
    let messaging = Arc::new(InMemoryMessaging::with_limits(limits));
    let ingress = Ingress::new(DynamicRouter::default(), "127.0.0.1:0".parse()?).await?;
    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(ingress))
        .with_plugin(messaging.clone())?
        .build()?
        .start()
        .await
        .context("host should start")?;

    let components = settings.msg_components;
    let mut workload_ids = Vec::with_capacity(components);
    for index in 0..components {
        let host_header = host_header(index);
        let workload_id = uuid::Uuid::new_v4().to_string();
        let config = HashMap::from([
            ("subscriptions".to_string(), SUBJECT.to_string()),
            (
                "admission_wait".to_string(),
                format!("{admission_wait_ms}ms"),
            ),
        ]);
        let response = host
            .workload_start(WorkloadStartRequest {
                workload_id: workload_id.clone(),
                workload: Workload {
                    namespace: "scale".to_string(),
                    name: host_header.clone(),
                    annotations: HashMap::new(),
                    service: None,
                    components: vec![Component {
                        name: "sleeper".to_string(),
                        digest: Some(digest.clone()),
                        bytes: wasm.clone(),
                        local_resources: LocalResources {
                            config: config.clone(),
                            ..Default::default()
                        },
                        pool_size: flavour.pool_size(),
                        ..Default::default()
                    }],
                    host_interfaces: vec![
                        http_interface(&host_header, "http_handler_p3"),
                        WitInterface {
                            namespace: "wasmcloud".to_string(),
                            package: "messaging".to_string(),
                            interfaces: ["handler".to_string()].into_iter().collect(),
                            version: Some(semver::Version::new(0, 3, 0)),
                            config,
                            name: None,
                        },
                    ],
                    volumes: vec![],
                },
            })
            .await
            .with_context(|| format!("starting messaging component {index}"))?;
        anyhow::ensure!(
            response.workload_status.workload_state == WorkloadState::Running,
            "messaging component {index} did not start: {}",
            response.workload_status.message
        );
        workload_ids.push(workload_id);
    }
    let workload_ids: Arc<[String]> = workload_ids.into();

    let baseline = measured_footprint(process_footprint(), "baseline")?;
    let (cpu_baseline, cpu_host_only) = host_cpu(&load.threads);
    let mut cpu_before = cpu_baseline;
    let pool_baseline = engine.pool_usage();
    let memories_total = pool_baseline.map(|u| u.memories.total);
    let core_instances_total = pool_baseline.map(|u| u.core_instances.total);
    let refusals_baseline = Refusals::now(&engine);
    let ceiling = settings.memory_limit / 100 * GUARD_SLACK_PERCENT;
    // A delivery holds at least one linear memory, whichever path builds it.
    let limits: BTreeMap<String, Option<u64>> = [
        ("messaging host ceiling", Some(host_total as u64)),
        (
            "messaging per-component ceilings",
            Some((per_component * components) as u64),
        ),
        ("pool memories", memories_total),
    ]
    .into_iter()
    .map(|(name, limit)| (name.to_string(), limit))
    .collect();
    let binding = limits.values().flatten().copied().min();

    let run_started = Instant::now();
    let mut steps: Vec<Step> = Vec::new();
    let mut offered = 0usize;
    let stop = loop {
        if settings
            .max_duration
            .is_some_and(|budget| run_started.elapsed() >= budget)
        {
            break Stop::TimeBudget;
        }
        if offered + settings.msg_step > settings.msg_max_offered {
            break Stop::Cap;
        }
        offered += settings.msg_step;

        let before = DELIVERIES.counts();
        DELIVERIES.reset_peak();
        let refusals_before = Refusals::now(&engine);
        let step_started = Instant::now();

        // Published from the load generator's runtime, round-robin across the
        // components, so the burst lands at once and its CPU is not the host's.
        // Yields how many were published before any error.
        let mut publisher = {
            let messaging = Arc::clone(&messaging);
            let workload_ids = Arc::clone(&workload_ids);
            let body = hold_ms.to_string().into_bytes();
            load.runtime.spawn(async move {
                let mut sent = 0;
                for workload_id in workload_ids.iter().cycle().take(offered) {
                    if let Err(e) = messaging.publish(workload_id, SUBJECT, body.clone()).await {
                        return (sent, Err(anyhow::anyhow!("publishing message {sent}: {e}")));
                    }
                    sent += 1;
                }
                (sent, Ok(()))
            })
        };

        // Every message ends one way or another well inside this: a burst
        // drains in waves of one hold each, at the ceiling at worst.
        let waves = offered.div_ceil(binding.unwrap_or(1).max(1) as usize) as u32;
        let settle_by = step_started + settings.msg_hold * (waves + 2) + Duration::from_secs(30);
        let mut peaks = Peaks::default();
        let mut published = None;
        let mut idle_since = None;
        loop {
            peaks.sample(&engine);
            if published.is_none() && publisher.is_finished() {
                published = Some(
                    (&mut publisher)
                        .await
                        .context("the publisher should not panic")?,
                );
            }
            idle_since = DELIVERIES
                .idle()
                .then(|| idle_since.unwrap_or_else(Instant::now));
            // Idle for a hold with messages unaccounted: an ending this bench
            // does not recognise, which still closed its delivery's span.
            if let (Some((sent, _)), Some(idle_since)) = (&published, idle_since)
                && (DELIVERIES.counts().since(before).ended() >= *sent
                    || idle_since.elapsed() >= settings.msg_hold)
            {
                break;
            }
            if Instant::now() >= settle_by {
                break;
            }
            tokio::time::sleep(SAMPLE_EVERY).await;
        }
        let (sent, publish_result) = match published {
            Some(published) => published,
            None => publisher.await.context("the publisher should not panic")?,
        };

        let counts = DELIVERIES.counts().since(before);
        let unaccounted = sent.saturating_sub(counts.ended());
        let peak_in_flight = DELIVERIES.peak.load(Ordering::SeqCst);
        let footprint = measured_footprint(peaks.footprint, "step")?;
        let refusals_now = Refusals::now(&engine);
        let in_step = refusals_now.since(refusals_before);
        let (cpu_now, _) = host_cpu(&load.threads);
        let wall_secs = step_started.elapsed().as_secs_f64().max(f64::EPSILON);
        let cpu_percent = cpu_now
            .zip(cpu_before)
            .and_then(|(now, before)| now.checked_sub(before))
            .map(|d| d.as_secs_f64() / (wall_secs * cpu.cores as f64) * 100.0);
        cpu_before = cpu_now;
        let failed = counts.failed + unaccounted + (offered - sent);
        let first = steps.is_empty();
        steps.push(Step {
            workloads: components,
            started: if first { components } else { 0 },
            refused: in_step.total(),
            footprint_bytes: Some(footprint),
            guest_memory_bytes: Some(peaks.guest_memory),
            core_instances_in_use: peaks.core_instances,
            core_instances_total,
            memories_in_use: peaks.memories,
            memories_total,
            cpu_percent,
            clean: in_step.total() == 0 && counts.shed == 0 && failed == 0 && counts.completed > 0,
            messages_offered: Some(offered),
            messages_peak_in_flight: Some(peak_in_flight),
            messages_completed: Some(counts.completed),
            messages_shed: Some(counts.shed),
            messages_failed: Some(failed),
            ..Default::default()
        });
        print_step(first, steps.last().expect("just pushed"));

        if let Some(stop) = memory_stop(
            &engine,
            refusals_now.since(refusals_baseline),
            footprint,
            ceiling,
        ) {
            break stop;
        }
        if let Err(e) = publish_result {
            break Stop::Requests(format!("{e:#}"));
        }
        if in_step.pool > 0 {
            break Stop::PoolRefused {
                refused: in_step.pool,
                shed: counts.shed,
            };
        }
        if counts.shed > 0 {
            break Stop::AdmissionShed {
                shed: counts.shed,
                in_flight: peak_in_flight,
            };
        }
        if failed > 0 {
            break Stop::Requests(format!(
                "{failed} of {offered} deliveries failed, {unaccounted} of them never reported how"
            ));
        }
    };

    let report = Report {
        bench: "scale_density".into(),
        flavour: Some(flavour),
        label: label(flavour.name(), cpu.limit),
        subject: format!(
            "{FIXTURE} x{components} (poolSize {}), {hold_ms}ms per delivery, admission wait \
             {admission_wait_ms}ms, ceilings {host_total} host / {per_component} per component, \
             pool {} memories / {} core instances, {} cores{}",
            flavour.pool_size(),
            memories_total.map_or_else(|| "-".into(), |n| n.to_string()),
            core_instances_total.map_or_else(|| "-".into(), |n| n.to_string()),
            cpu.cores,
            if cpu_host_only {
                ""
            } else {
                ", CPU includes the publisher's"
            },
        ),
        cores: cpu.cores,
        memory_limit_bytes: settings.memory_limit,
        baseline_footprint_bytes: Some(baseline),
        limits,
        cpu_secs_total: host_cpu(&load.threads)
            .0
            .zip(cpu_baseline)
            .map(|(now, before)| now.saturating_sub(before).as_secs_f64()),
        stop: stop.to_string(),
        steps,
        ..Default::default()
    };
    eprintln!(
        "stopped at {offered} offered, {} in flight, footprint {}: {}",
        report.peak_in_flight().unwrap_or(0),
        report
            .steps
            .last()
            .and_then(|s| s.footprint_bytes)
            .map_or_else(|| "-".into(), render_bytes),
        report.stop
    );
    for workload_id in workload_ids.iter() {
        let _ = host
            .workload_stop(WorkloadStopRequest {
                workload_id: workload_id.clone(),
            })
            .await;
    }
    let _ = host.stop().await;
    Ok(report)
}

#[cfg(test)]
mod tests {
    // As in `scale_density.rs`: unused where `harness = false` drops the tests.
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn plugin_messages_classify_how_a_delivery_ended() {
        assert_eq!(Ending::of(COMPLETED), Some(Ending::Completed));
        assert_eq!(
            Ending::of(
                "no messaging admission slot came free before the deadline; dropping message"
            ),
            Some(Ending::Shed)
        );
        assert_eq!(
            Ending::of("failed to instantiate component c: pool exhausted"),
            Some(Ending::Failed)
        );
        assert_eq!(
            Ending::of("Tracking component in in-memory messaging"),
            None
        );
    }

    #[test]
    fn the_messages_and_span_counted_are_the_plugins_own() {
        let source = [
            include_str!("../../src/plugin/wasmcloud_messaging/mod.rs"),
            include_str!("../../src/plugin/wasmcloud_messaging/in_memory.rs"),
        ]
        .concat();
        for needle in [COMPLETED, SHED, DELIVERY_SPAN].into_iter().chain(FAILED) {
            assert!(
                source.contains(needle),
                "the plugin no longer logs {needle:?}"
            );
        }
    }

    #[test]
    fn a_delivery_is_in_flight_until_its_span_closes() {
        let deliveries = Deliveries::default();
        deliveries.admitted();
        deliveries.admitted();
        deliveries.ended(Ending::Shed);
        deliveries.left();
        deliveries.ended(Ending::Completed);
        assert!(!deliveries.idle());
        deliveries.left();
        assert!(deliveries.idle());
        assert_eq!(deliveries.peak.load(Ordering::SeqCst), 2);
        assert_eq!(
            deliveries.counts(),
            Counts {
                completed: 1,
                shed: 1,
                failed: 0
            }
        );
    }
}
