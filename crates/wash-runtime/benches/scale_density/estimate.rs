//! Estimate for the density experiment.
use super::{Settings, Step};
use wash_runtime::engine::pool_usage::PoolUsage;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum CpuCapacity {
    #[default]
    IdleUnbounded,
}

/// What each measured constraint predicts. `None` means unmeasured or
/// insufficient evidence for an estimate, rendered as `?` in the report.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Limits {
    /// Workloads max-process-memory fits, at the marginal cost per workload.
    pub(super) memory: Option<u64>,
    /// Workloads max-guest-memory fits, at the guest charge per workload.
    pub(super) guest_memory: Option<u64>,
    /// Workloads the pool's core instances and linear memories fit.
    pub(super) pool_core_instances: Option<u64>,
    pub(super) pool_memories: Option<u64>,
    /// Idle capacity is unbounded by CPU; throughput still needs measurements.
    pub(super) cpu: CpuCapacity,
    /// The figures the limits are built from.
    pub(super) marginal_bytes_per_workload: Option<u64>,
    pub(super) guest_bytes_per_workload: Option<u64>,
    pub(super) core_instances_per_workload: Option<u64>,
    pub(super) memories_per_workload: Option<u64>,
    /// What the CPU limit allows: starts a second, and requests a second
    /// spread across the set, at the CPU each cost.
    pub(super) max_starts_per_sec: Option<f64>,
    pub(super) max_requests_per_sec: Option<f64>,
}

impl Limits {
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
        let (Some(first), Some(last)) = (served.first(), served.last()) else {
            return Self::default();
        };
        let workloads = last.workloads as u64;
        // The slope between the first and last steps that served leaves out
        // what the first workload paid once for everyone (the compile, the
        // shared linkers), which an average over few workloads would count.
        let marginal = if last.workloads > first.workloads {
            last.footprint_bytes.saturating_sub(first.footprint_bytes)
                / (last.workloads - first.workloads) as u64
        } else {
            last.footprint_bytes.saturating_sub(baseline) / workloads
        };
        let memory = (marginal > 0).then(|| {
            let headroom = settings.memory_limit as i128 - last.footprint_bytes as i128;
            (workloads as i128 + headroom / marginal as i128).max(0) as u64
        });
        let per = |held: u64| (held > 0).then_some(held);
        let guest_per = per(last.guest_memory_bytes / workloads);
        let core_per = per(last
            .core_instances_in_use
            .saturating_sub(pool_baseline.map_or(0, |u| u.core_instances.in_use))
            / workloads);
        let memories_per = per(last
            .memories_in_use
            .saturating_sub(pool_baseline.map_or(0, |u| u.memories.in_use))
            / workloads);
        let rate = |cpu: fn(&Step) -> Option<f64>, count: fn(&Step) -> usize| {
            measured_rate(cores, served.iter().map(|s| (cpu(s), count(s))))
        };
        Self {
            memory,
            guest_memory: guest_per.map(|per| settings.guest_memory / per),
            pool_core_instances: core_per.map(|per| last.core_instances_total / per),
            pool_memories: memories_per.map(|per| last.memories_total / per),
            cpu: CpuCapacity::IdleUnbounded,
            marginal_bytes_per_workload: Some(marginal),
            guest_bytes_per_workload: guest_per,
            core_instances_per_workload: core_per,
            memories_per_workload: memories_per,
            max_starts_per_sec: rate(|s| s.start_cpu_secs, |s| s.started_in_step),
            max_requests_per_sec: rate(|s| s.sample_cpu_secs, |s| s.requests_ok),
        }
    }

    /// The constraint that binds first, and the count it allows.
    pub(super) fn binding(&self) -> (&'static str, Option<u64>) {
        [
            ("max-process-memory", self.memory),
            ("max-guest-memory", self.guest_memory),
            ("pool core instances", self.pool_core_instances),
            ("pool memories", self.pool_memories),
        ]
        .into_iter()
        .filter_map(|(name, limit)| limit.map(|n| (name, Some(n))))
        .min_by_key(|(_, n)| *n)
        .unwrap_or(("none", None))
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

/// Missing measurements are unknown, not evidence of unlimited capacity.
pub(super) fn or_unknown(limit: Option<u64>) -> String {
    limit.map_or_else(|| "?".to_string(), |n| n.to_string())
}
