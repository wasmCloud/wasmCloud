//! The record both density benchmarks write (`wash-runtime`'s in-process
//! `scale_density` and `bench-scale` over NATS), the sampling rules that
//! decide what counts as served, one text renderer, and the named metrics
//! `bench-tools jsonl` publishes from it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flavour {
    /// `pool_size: 1`: every workload keeps an instance parked.
    Warm,
    /// `pool_size: 0`: a request instantiates and then drops one.
    ScaleToZero,
}

impl Flavour {
    pub const ALL: [Self; 2] = [Self::Warm, Self::ScaleToZero];

    pub fn name(self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::ScaleToZero => "scale-to-zero",
        }
    }

    pub fn pool_size(self) -> i32 {
        match self {
            Self::Warm => 1,
            Self::ScaleToZero => 0,
        }
    }
}

/// One measurement, taken after a step of starts. `None` is a reading the
/// bench did not or could not take, never zero.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    /// Running workloads after this step, and how many this step started.
    pub workloads: usize,
    pub started: usize,
    pub starts_per_sec: f64,
    pub start_p99_ms: Option<f64>,
    /// Starts, instantiations or growths refused during this step.
    pub refused: u64,
    pub footprint_bytes: Option<u64>,
    /// Footprint over the baseline, per workload.
    pub bytes_per_workload: Option<u64>,
    pub guest_memory_bytes: Option<u64>,
    pub core_instances_in_use: Option<u64>,
    pub core_instances_total: Option<u64>,
    pub memories_in_use: Option<u64>,
    pub memories_total: Option<u64>,
    pub requests_ok: usize,
    pub requests_failed: usize,
    /// Warm: first requests to this step's new workloads that failed.
    pub warmup_failed: usize,
    pub requests_per_sec: f64,
    pub request_p50_ms: Option<f64>,
    pub request_p99_ms: Option<f64>,
    /// Scale-to-zero: how long until the sample's instances were gone.
    pub spin_down_ms: Option<f64>,
    /// Host CPU over the step, as a share of the cores it may use, and the
    /// part spent starting and serving the sample.
    pub cpu_percent: Option<f64>,
    pub start_cpu_secs: Option<f64>,
    pub sample_cpu_secs: Option<f64>,
    /// Every start succeeded, nothing was refused, and the sample served
    /// (see [`ServingEvidence`]). Only clean steps say what a workload costs.
    pub clean: bool,
    /// The workload count the measurements so far say the host can hold.
    pub theoretical_limit: Option<u64>,
}

/// One flavour's run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    /// The bench that wrote it: `scale_density` or `bench-scale`.
    pub bench: String,
    pub flavour: Option<Flavour>,
    /// The column heading: the flavour, and its CPU limit when it has one.
    pub label: String,
    /// What ran: the fixture or image, and the host when it is not in-process.
    pub subject: String,
    pub cores: usize,
    pub memory_limit_bytes: u64,
    pub baseline_footprint_bytes: Option<u64>,
    /// What each constraint predicts the host holds, by constraint name;
    /// `None` when there was not enough evidence to estimate it.
    pub limits: BTreeMap<String, Option<u64>>,
    pub marginal_bytes_per_workload: Option<u64>,
    /// What the CPU allows: starts a second, and requests a second spread
    /// across the set, at the CPU each cost.
    pub cpu_max_starts_per_sec: Option<f64>,
    pub cpu_max_requests_per_sec: Option<f64>,
    pub cpu_secs_total: Option<f64>,
    /// Why the run stopped.
    pub stop: String,
    pub steps: Vec<Step>,
}

impl Report {
    /// Workloads at the last clean step.
    pub fn served(&self) -> usize {
        self.last_clean().map_or(0, |s| s.workloads)
    }

    /// Workloads running when the run stopped.
    pub fn achieved(&self) -> usize {
        self.steps.last().map_or(0, |s| s.workloads)
    }

    pub fn last_clean(&self) -> Option<&Step> {
        self.steps.iter().rev().find(|s| s.clean)
    }

    /// The constraint that binds first, and the count it allows.
    pub fn binding(&self) -> Option<(&str, u64)> {
        self.limits
            .iter()
            .filter_map(|(name, limit)| Some((name.as_str(), (*limit)?)))
            .min_by_key(|(_, n)| *n)
    }

    pub fn mean_starts_per_sec(&self) -> Option<f64> {
        let rates: Vec<f64> = self.steps.iter().map(|s| s.starts_per_sec).collect();
        (!rates.is_empty()).then(|| rates.iter().sum::<f64>() / rates.len() as f64)
    }

    /// CPU spent starting, per start, over the steps where it was read.
    pub fn cpu_ms_per_start(&self) -> Option<f64> {
        let (secs, starts) = self
            .steps
            .iter()
            .filter_map(|s| s.start_cpu_secs.map(|cpu| (cpu, s.started)))
            .fold((0.0, 0), |(secs, n), (cpu, started)| {
                (secs + cpu, n + started)
            });
        (starts > 0).then(|| secs * 1000.0 / starts as f64)
    }

    /// The figures published to the trend site, each with its unit and
    /// which direction is better. Unmeasured figures are left out.
    pub fn metrics(&self) -> Vec<Metric> {
        let clean = self.last_clean();
        let count = |n: Option<u64>| n.map(|n| n as f64);
        [
            (
                "served_workloads",
                Some(self.served() as f64),
                "workloads",
                Better::Higher,
            ),
            (
                "binding_limit",
                count(self.binding().map(|(_, n)| n)),
                "workloads",
                Better::Higher,
            ),
            (
                "marginal_bytes_per_workload",
                count(self.marginal_bytes_per_workload),
                "B",
                Better::Lower,
            ),
            (
                "baseline_footprint_bytes",
                count(self.baseline_footprint_bytes),
                "B",
                Better::Lower,
            ),
            (
                "starts_per_sec",
                self.mean_starts_per_sec(),
                "ops/s",
                Better::Higher,
            ),
            (
                "start_p99_ms",
                clean.and_then(|s| s.start_p99_ms),
                "ms",
                Better::Lower,
            ),
            (
                "request_p99_ms",
                clean.and_then(|s| s.request_p99_ms),
                "ms",
                Better::Lower,
            ),
            (
                "spin_down_ms",
                clean.and_then(|s| s.spin_down_ms),
                "ms",
                Better::Lower,
            ),
            (
                "cpu_ms_per_start",
                self.cpu_ms_per_start(),
                "ms",
                Better::Lower,
            ),
        ]
        .into_iter()
        .filter_map(|(name, value, unit, better)| {
            Some(Metric {
                name,
                value: value.filter(|v| v.is_finite())?,
                unit,
                better,
            })
        })
        .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Better {
    Higher,
    Lower,
}

impl Better {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Higher => "higher",
            Self::Lower => "lower",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub name: &'static str,
    pub value: f64,
    pub unit: &'static str,
    pub better: Better,
}

/// What a workload adds to the footprint, and how many workloads fit
/// `memory_limit` at that cost, from the clean steps with a reading: the
/// slope between the first and last, which leaves out what the first
/// workload paid once for everyone (the compile, the shared linkers), or the
/// average over `baseline` when there is only one. `None` without a clean
/// step; the count is `None` when a workload added nothing measurable.
pub fn memory_estimate(
    steps: &[Step],
    baseline: u64,
    memory_limit: u64,
) -> Option<(u64, Option<u64>)> {
    let mut clean = steps
        .iter()
        .filter(|s| s.clean && s.workloads > 0)
        .filter_map(|s| Some((s.workloads as u64, s.footprint_bytes?)));
    let first = clean.next()?;
    let (n, footprint) = clean.next_back().unwrap_or(first);
    let marginal = if n > first.0 {
        footprint.saturating_sub(first.1) / (n - first.0)
    } else {
        footprint.saturating_sub(baseline) / n
    };
    let fits = (marginal > 0).then(|| {
        let headroom = memory_limit as i128 - footprint as i128;
        (n as i128 + headroom / marginal as i128).max(0) as u64
    });
    Some((marginal, fits))
}

/// A capacity claim needs a positive observation and no failed requests.
/// A zero-request step is unmeasured, even if all workload starts succeeded.
pub fn served_sample(successes: usize, failures: usize, warmup_failures: usize) -> bool {
    successes > 0 && failures == 0 && warmup_failures == 0
}

/// A later sample may omit a workload whose initial warm-up failed. Such a
/// sample cannot establish warm capacity until that workload is verified; the
/// density runners stop instead of silently forgetting the failed warm-up.
#[derive(Default)]
pub struct ServingEvidence {
    warmup_failed: bool,
}

impl ServingEvidence {
    pub fn observe(&mut self, successes: usize, failures: usize, warmup_failures: usize) -> bool {
        self.warmup_failed |= warmup_failures > 0;
        served_sample(successes, failures, usize::from(self.warmup_failed))
    }
}

/// Validate the sample size before allocating workloads.
pub fn sample_size(value: usize) -> Result<usize, &'static str> {
    if value == 0 {
        Err("sample size must be at least 1 to measure served capacity")
    } else {
        Ok(value)
    }
}

/// Distinct, ascending positions spanning the oldest and newest workloads.
/// A single observation targets the newest workload, where start failures are
/// most likely to be hidden by sampling only an already healthy older batch.
pub fn spread(count: usize, requested: usize) -> Vec<usize> {
    let n = requested.min(count);
    match n {
        0 => Vec::new(),
        1 => vec![count - 1],
        _ => (0..n)
            .map(|i| ((i as u128 * (count - 1) as u128) / (n - 1) as u128) as usize)
            .collect(),
    }
}

/// The `p` quantile (0.0..=1.0) of `values` by nearest rank, or `None` for
/// no values. Sorts a copy, so callers can pass samples in arrival order.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((sorted.len() - 1) as f64 * p.clamp(0.0, 1.0)).round() as usize;
    sorted.get(rank).copied()
}

/// Bytes with a binary unit, as the runtime prints its limits.
pub fn render_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    for (unit, suffix) in [
        (KIB << 30, "TiB"),
        (KIB << 20, "GiB"),
        (KIB << 10, "MiB"),
        (KIB, "KiB"),
    ] {
        if bytes >= unit {
            return format!("{:.1}{suffix}", bytes as f64 / unit as f64);
        }
    }
    format!("{bytes}B")
}

/// `-` is a reading not taken; `?` (for limits) is too little evidence.
/// Neither means unlimited.
fn or_dash<T>(value: Option<T>, show: impl Fn(T) -> String) -> String {
    value.map_or_else(|| "-".into(), show)
}

fn ms(value: Option<f64>) -> String {
    or_dash(value, |v| format!("{v:.1}"))
}

/// One row of the live step table, on stderr; `header` first.
pub fn print_step(header: bool, step: &Step) {
    if header {
        eprintln!(
            "{:>9} {:>8} {:>10} {:>9} {:>10} {:>13} {:>9} {:>7} {:>8} {:>8} {:>7} {:>7} {:>5} {:>8}",
            "workloads",
            "starts/s",
            "footprint",
            "per-wkld",
            "guest mem",
            "core inst",
            "ok/fail",
            "req/s",
            "p50 ms",
            "p99 ms",
            "down ms",
            "refused",
            "cpu%",
            "limit",
        );
    }
    eprintln!(
        "{:>9} {:>8.0} {:>10} {:>9} {:>10} {:>13} {:>9} {:>7.0} {:>8} {:>8} {:>7} {:>7} {:>5} {:>8}",
        step.workloads,
        step.starts_per_sec,
        or_dash(step.footprint_bytes, render_bytes),
        or_dash(step.bytes_per_workload, render_bytes),
        or_dash(step.guest_memory_bytes, render_bytes),
        or_dash(step.core_instances_in_use, |n| format!(
            "{n}/{}",
            or_dash(step.core_instances_total, |t| t.to_string())
        )),
        format!("{}/{}", step.requests_ok, step.requests_failed),
        step.requests_per_sec,
        ms(step.request_p50_ms),
        ms(step.request_p99_ms),
        or_dash(step.spin_down_ms, |v| format!("{v:.0}")),
        step.refused,
        or_dash(step.cpu_percent, |v| format!("{v:.0}")),
        step.theoretical_limit
            .map_or_else(|| "?".into(), |n| n.to_string()),
    );
}

/// The flavours side by side, one row per measure, on stderr. A row no
/// report measured is left out.
pub fn print_comparison(reports: &[Report]) {
    let Some(first) = reports.first() else {
        return;
    };
    eprintln!(
        "\n== {}: {}, limit {} ==",
        first.bench,
        first.subject,
        render_bytes(first.memory_limit_bytes)
    );
    let row = |label: &str, cell: &dyn Fn(&Report) -> String| {
        let cells: Vec<String> = reports.iter().map(cell).collect();
        if cells.iter().any(|c| c != "-") {
            let cells: Vec<String> = cells.iter().map(|c| format!("{c:>20}")).collect();
            eprintln!("{label:<26}{}", cells.join(""));
        }
    };
    let names: std::collections::BTreeSet<&String> =
        reports.iter().flat_map(|r| r.limits.keys()).collect();
    row("", &|r| r.label.clone());
    row("cores", &|r| r.cores.to_string());
    row("served workloads", &|r| r.served().to_string());
    row("achieved workloads", &|r| r.achieved().to_string());
    for name in names {
        row(&format!("limit: {name}"), &|r| match r.limits.get(name) {
            Some(Some(n)) => n.to_string(),
            Some(None) => "?".into(),
            None => "-".into(),
        });
    }
    row("binding limit", &|r| {
        r.binding()
            .map_or_else(|| "?".into(), |(name, n)| format!("{n} ({name})"))
    });
    row("marginal per workload", &|r| {
        or_dash(r.marginal_bytes_per_workload, render_bytes)
    });
    row("baseline footprint", &|r| {
        or_dash(r.baseline_footprint_bytes, render_bytes)
    });
    row("final footprint", &|r| {
        or_dash(r.steps.last().and_then(|s| s.footprint_bytes), render_bytes)
    });
    row("guest memory charged", &|r| {
        or_dash(
            r.steps.last().and_then(|s| s.guest_memory_bytes),
            render_bytes,
        )
    });
    row("starts/s (mean)", &|r| {
        or_dash(r.mean_starts_per_sec(), |v| format!("{v:.0}"))
    });
    row("start p99 ms (served)", &|r| {
        ms(r.last_clean().and_then(|s| s.start_p99_ms))
    });
    row("req/s (served)", &|r| {
        or_dash(r.last_clean(), |s| format!("{:.0}", s.requests_per_sec))
    });
    row("p50 / p99 ms (served)", &|r| {
        or_dash(r.last_clean(), |s| {
            format!("{} / {}", ms(s.request_p50_ms), ms(s.request_p99_ms))
        })
    });
    row("spin-down ms (served)", &|r| {
        or_dash(r.last_clean().and_then(|s| s.spin_down_ms), |v| {
            format!("{v:.0}")
        })
    });
    row("cpu max starts/s", &|r| {
        or_dash(r.cpu_max_starts_per_sec, |v| format!("{v:.0}"))
    });
    row("cpu max req/s", &|r| {
        or_dash(r.cpu_max_requests_per_sec, |v| format!("{v:.0}"))
    });
    // Only the CPU spent starting, not the samples and spin-downs between.
    row("cpu-ms per start", &|r| {
        or_dash(r.cpu_ms_per_start(), |v| format!("{v:.2}"))
    });
    row("cpu-s total", &|r| {
        or_dash(r.cpu_secs_total, |v| format!("{v:.1}"))
    });
    for report in reports {
        eprintln!("{}: stopped by {}", report.label, report.stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmeasured_or_failed_steps_are_not_served_capacity() {
        assert!(sample_size(0).is_err());
        assert_eq!(sample_size(1), Ok(1));
        assert!(!served_sample(0, 0, 0));
        assert!(!served_sample(1, 1, 0));
        assert!(!served_sample(100, 0, 1));
        assert!(served_sample(1, 0, 0));
    }

    #[test]
    fn a_later_sample_cannot_clear_an_unverified_warmup() {
        // One failed warm-up can be missed by both the current and next
        // sample, so checking only the current batch would claim 500 warm
        // workloads even though workload 4 never parked an instance.
        assert!(!spread(250, 200).contains(&4));
        assert!(!spread(500, 200).contains(&4));
        let mut evidence = ServingEvidence::default();
        assert!(!evidence.observe(200, 0, 1));
        assert!(!evidence.observe(200, 0, 0));

        let mut evidence = ServingEvidence::default();
        assert!(evidence.observe(200, 0, 0));
        assert!(!evidence.observe(200, 0, 1));
        assert!(!evidence.observe(200, 0, 0));
    }

    #[test]
    fn samples_reach_the_newest_workload_without_overflow() {
        assert!(spread(0, 5).is_empty());
        assert!(spread(5, 0).is_empty());
        assert_eq!(spread(100, 1), vec![99]);
        assert_eq!(spread(100, 4), vec![0, 33, 66, 99]);
        assert_eq!(spread(3, 10), vec![0, 1, 2]);
        let large = spread(usize::MAX, 100);
        assert_eq!(large.first(), Some(&0));
        assert_eq!(large.last(), Some(&(usize::MAX - 1)));
        assert!(large.windows(2).all(|pair| matches!(pair, [a, b] if a < b)));
    }

    #[test]
    fn percentile_by_nearest_rank() {
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[7.0], 0.99), Some(7.0));
        let values: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        assert_eq!(percentile(&values, 0.0), Some(1.0));
        assert_eq!(percentile(&values, 0.5), Some(51.0));
        assert_eq!(percentile(&values, 1.0), Some(100.0));
    }

    #[test]
    fn served_and_published_figures_come_from_the_last_clean_step() {
        let step = |workloads, clean, p99| Step {
            workloads,
            started: 10,
            starts_per_sec: 100.0,
            request_p99_ms: Some(p99),
            start_cpu_secs: Some(0.01),
            clean,
            ..Default::default()
        };
        let report = Report {
            limits: BTreeMap::from([
                ("memory".into(), Some(900)),
                ("pool".into(), Some(400)),
                ("guest".into(), None),
            ]),
            marginal_bytes_per_workload: Some(4096),
            steps: vec![
                step(10, true, 2.0),
                step(20, true, 3.0),
                step(30, false, 0.0),
            ],
            ..Default::default()
        };
        assert_eq!(report.served(), 20);
        assert_eq!(report.achieved(), 30);
        assert_eq!(report.binding(), Some(("pool", 400)));
        let metrics = report.metrics();
        let value = |name| metrics.iter().find(|m| m.name == name).map(|m| m.value);
        assert_eq!(value("served_workloads"), Some(20.0));
        assert_eq!(value("binding_limit"), Some(400.0));
        assert_eq!(value("request_p99_ms"), Some(3.0));
        assert_eq!(value("cpu_ms_per_start"), Some(1.0));
        // Unmeasured figures are left out, not published as zero.
        assert_eq!(value("baseline_footprint_bytes"), None);
        assert_eq!(value("spin_down_ms"), None);
        assert!(
            Report::default()
                .metrics()
                .iter()
                .all(|m| m.name == "served_workloads")
        );
    }

    #[test]
    fn memory_estimates_use_the_marginal_cost_of_clean_steps() {
        let step = |workloads, footprint, clean| Step {
            workloads,
            footprint_bytes: Some(footprint),
            clean,
            ..Default::default()
        };
        // 100 MiB paid once by the first step, then 1 KiB a workload.
        let steps = [
            step(100, 200 << 20, true),
            step(200, (200 << 20) + 100 * 1024, true),
            step(300, 900 << 20, false),
        ];
        let (marginal, fits) = memory_estimate(&steps, 100 << 20, 300 << 20).unwrap();
        assert_eq!(marginal, 1024);
        assert_eq!(fits, Some(200 + ((100 << 20) - 100 * 1024) / 1024));
        // One clean step: the average over the baseline.
        assert_eq!(
            memory_estimate(&steps[..1], 100 << 20, 300 << 20)
                .unwrap()
                .0,
            1 << 20
        );
        assert_eq!(memory_estimate(&steps[2..], 0, 1), None);
        assert_eq!(memory_estimate(&[step(1, 5, true)], 5, 10), Some((0, None)));
    }

    #[test]
    fn reports_round_trip_and_tolerate_missing_fields() {
        let report: Report =
            serde_json::from_str(r#"{"bench":"bench-scale","flavour":"warm"}"#).unwrap();
        assert_eq!(report.flavour, Some(Flavour::Warm));
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back.bench, "bench-scale");
        assert_eq!(render_bytes(512), "512B");
        assert_eq!(render_bytes(3 << 29), "1.5GiB");
    }
}
