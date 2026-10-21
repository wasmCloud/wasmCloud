//! Estimate for the density experiment.
use std::collections::BTreeMap;

use super::{Settings, Step};
use bench_tools::density::memory_estimate;
use wash_runtime::engine::pool_usage::PoolUsage;

/// What each measured constraint predicts, from the clean steps so far.
/// `None` means too little evidence for an estimate, not unlimited.
#[derive(Debug, Clone, Default)]
pub(super) struct Estimate {
    /// Workloads each constraint fits: max-process-memory at the marginal
    /// cost per workload, max-guest-memory at the guest charge, and the
    /// pool's core instances and linear memories.
    pub(super) limits: BTreeMap<String, Option<u64>>,
    pub(super) marginal_bytes_per_workload: Option<u64>,
    /// What the CPU limit allows: starts a second, and requests a second
    /// spread across the set, at the CPU each cost.
    pub(super) max_starts_per_sec: Option<f64>,
    pub(super) max_requests_per_sec: Option<f64>,
}

impl Estimate {
    pub(super) fn from_steps(
        settings: &Settings,
        cores: usize,
        baseline: u64,
        pool_baseline: &Option<PoolUsage>,
        steps: &[Step],
    ) -> Self {
        // Only clean steps: once anything is refused, a full pool or a guard
        // holding growth back distorts what a workload appears to cost.
        let served: Vec<&Step> = steps
            .iter()
            .filter(|s| s.workloads > 0 && s.clean)
            .collect();
        let Some(last) = served.last() else {
            return Self::default();
        };
        let workloads = last.workloads as u64;
        let reading = |value: Option<u64>| value.unwrap_or(0);
        let (marginal, memory) = memory_estimate(steps, baseline, settings.memory_limit).unzip();
        let per = |held: u64| (held > 0).then_some(held);
        let guest_per = per(reading(last.guest_memory_bytes) / workloads);
        let core_per = per(reading(last.core_instances_in_use)
            .saturating_sub(pool_baseline.map_or(0, |u| u.core_instances.in_use))
            / workloads);
        let memories_per = per(reading(last.memories_in_use)
            .saturating_sub(pool_baseline.map_or(0, |u| u.memories.in_use))
            / workloads);
        let rate = |cpu: fn(&Step) -> Option<f64>, count: fn(&Step) -> usize| {
            measured_rate(cores, served.iter().map(|s| (cpu(s), count(s))))
        };
        Self {
            limits: BTreeMap::from([
                ("max-process-memory".into(), memory.flatten()),
                (
                    "max-guest-memory".into(),
                    guest_per.map(|per| settings.guest_memory / per),
                ),
                (
                    "pool core instances".into(),
                    core_per.map(|per| reading(last.core_instances_total) / per),
                ),
                (
                    "pool memories".into(),
                    memories_per.map(|per| reading(last.memories_total) / per),
                ),
            ]),
            marginal_bytes_per_workload: marginal,
            max_starts_per_sec: rate(|s| s.start_cpu_secs, |s| s.started),
            max_requests_per_sec: rate(|s| s.sample_cpu_secs, |s| s.requests_ok),
        }
    }

    /// The count the first constraint to bind allows.
    pub(super) fn binding(&self) -> Option<u64> {
        self.limits.values().flatten().copied().min()
    }
}

/// Only count operations whose CPU observation is available. Missing CPU
/// readings must not contribute work to a zero-duration denominator.
pub(super) fn measured_rate(
    cores: usize,
    samples: impl Iterator<Item = (Option<f64>, usize)>,
) -> Option<f64> {
    let (seconds, operations) = samples
        .filter_map(|(cpu, n)| cpu.map(|cpu| (cpu, n)))
        .filter(|(cpu, _)| cpu.is_finite() && *cpu > 0.0)
        .fold((0.0, 0usize), |(secs, count), (cpu, n)| {
            (secs + cpu, count + n)
        });
    (seconds > 0.0 && operations > 0).then(|| cores as f64 * operations as f64 / seconds)
}
