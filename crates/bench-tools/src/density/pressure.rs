//! Evidence and outcome checks for container memory pressure runs.

use serde::{Deserialize, Serialize};

use super::Report;

pub const MIB: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    Sparse,
    DataHeavy,
    Multiple,
    Dirty,
}

impl Shape {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sparse => "sparse",
            Self::DataHeavy => "data-heavy",
            Self::Multiple => "multiple",
            Self::Dirty => "dirty",
        }
    }

    /// Memories, initial bytes per memory, and active data bytes per memory.
    pub fn dimensions(self, dirty_mib: u64) -> (u64, u64, u64) {
        match self {
            Self::Sparse => (1, MIB, 4096),
            Self::DataHeavy => (1, 8 * MIB, 8 * MIB),
            Self::Multiple => (4, MIB, 4096),
            Self::Dirty => (1, dirty_mib * MIB, 4096),
        }
    }
}

impl std::str::FromStr for Shape {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        <Self as clap::ValueEnum>::from_str(raw, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Baseline,
    Ready,
    Writing,
    Touched,
    Released,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Ready => "ready",
            Self::Writing => "writing",
            Self::Touched => "touched",
            Self::Released => "released",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Completed,
    ExpectedOom,
    Failed,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::ExpectedOom => "expected late-write OOM",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub shape: Shape,
    pub cow: bool,
    pub initial_bytes: u64,
    pub memories: u64,
    pub swap_limit: u64,
    pub guest_limit: u64,
    pub initial_limit: u64,
    pub process_limit: u64,
    pub architecture: String,
    pub kernel: Option<String>,
    pub transparent_hugepage: Option<String>,
    pub outcome: Option<Outcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub phase: Phase,
    pub cgroup_current: Option<u64>,
    pub cgroup_peak: Option<u64>,
    pub guest_refused: u64,
    pub initial_refused: u64,
    pub process_refused: u64,
    pub process_over: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
    pub memory_mib: u64,
    pub shape: Shape,
    pub mode: String,
    pub cow: bool,
    pub dirty_mib: u64,
    pub guest_memory_mib: Option<u64>,
    pub initial_cap_mib: Option<u64>,
    pub repeat: usize,
}

impl Case {
    pub fn label(&self) -> String {
        format!(
            "{}m-{}-{}-cow-{}-{}",
            self.memory_mib,
            self.shape.name(),
            self.mode,
            self.cow,
            self.repeat
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Exit {
    pub exit_code: i32,
    #[serde(rename = "OOMKilled")]
    pub oom_killed: bool,
}

/// A late-write OOM requires phase, accounting, and Docker evidence.
pub fn classify(case: &Case, exit: &Exit, report: &Report) -> Result<Outcome, String> {
    let config = report
        .pressure
        .as_ref()
        .ok_or("missing pressure settings")?;
    let ready = report
        .steps
        .iter()
        .find(|s| s.pressure.as_ref().is_some_and(|p| p.phase == Phase::Ready))
        .ok_or("failed before admission completed")?;
    let sample = ready
        .pressure
        .as_ref()
        .ok_or("missing admission evidence")?;
    let guest_limit = case
        .guest_memory_mib
        .map_or((case.memory_mib * MIB * 3 / 4).max(256 * MIB), |m| m * MIB);
    let initial_limit = case.initial_cap_mib.map_or(4 * guest_limit, |m| m * MIB);
    if report.guest_memory_mode.as_deref() != Some(case.mode.as_str())
        || config.cow != case.cow
        || config.shape != case.shape
        || report.memory_limit_bytes != case.memory_mib * MIB
        || config.swap_limit != 0
        || config.guest_limit != guest_limit
        || (case.mode == "growth" && config.initial_limit != initial_limit)
    {
        return Err("reported settings differ from requested settings".into());
    }
    if ready.workloads == 0 {
        return Err("no guests admitted".into());
    }
    let (memories, per_memory, _) = case.shape.dimensions(case.dirty_mib);
    if config.memories != memories || config.initial_bytes != memories * per_memory {
        return Err("fixture has the wrong memory shape".into());
    }
    let total = ready.workloads as u64 * memories * per_memory;
    let uncharged = if case.mode == "growth" {
        ready.workloads as u64 * memories * per_memory.min(2 * MIB)
    } else {
        0
    };
    if ready.guest_memory_bytes != Some(total - uncharged)
        || ready.initial_memory_bytes != Some(uncharged)
    {
        return Err("guest reservations differ from retained memories".into());
    }
    if total - uncharged > config.guest_limit || uncharged > config.initial_limit {
        return Err("guest reservations exceeded their budgets".into());
    }
    if !sample.process_over
        && sample.guest_refused + sample.initial_refused + sample.process_refused == 0
    {
        return Err("admission stopped without memory-pressure evidence".into());
    }
    let phases: Vec<_> = report
        .steps
        .iter()
        .map(|s| s.pressure.as_ref().map(|p| p.phase))
        .collect();
    if exit.oom_killed {
        if case.mode == "growth"
            && case.shape == Shape::Dirty
            && exit.exit_code == 137
            && phases
                == [
                    Some(Phase::Baseline),
                    Some(Phase::Ready),
                    Some(Phase::Writing),
                ]
            && total > report.memory_limit_bytes
        {
            return Ok(Outcome::ExpectedOom);
        }
        return Err("unexpected OOM (admission or control case)".into());
    }
    if exit.exit_code != 0 {
        return Err(format!(
            "process exited {} without verified OOM",
            exit.exit_code
        ));
    }
    if phases
        != [
            Some(Phase::Baseline),
            Some(Phase::Ready),
            Some(Phase::Writing),
            Some(Phase::Touched),
            Some(Phase::Released),
        ]
    {
        return Err("missing or out-of-order completion samples".into());
    }
    let touched = report
        .steps
        .iter()
        .find(|s| {
            s.pressure
                .as_ref()
                .is_some_and(|p| p.phase == Phase::Touched)
        })
        .ok_or("missing retained-memory sample")?;
    if touched.workloads != ready.workloads
        || touched.guest_memory_bytes != ready.guest_memory_bytes
        || touched.initial_memory_bytes != ready.initial_memory_bytes
    {
        return Err("retained memories or reservations changed during writes".into());
    }
    let released = report.steps.last().ok_or("missing cleanup sample")?;
    if released.workloads != 0
        || released.guest_memory_bytes != Some(0)
        || released.initial_memory_bytes != Some(0)
    {
        return Err("guest accounting did not return to zero".into());
    }
    if case.shape == Shape::Dirty && total > report.memory_limit_bytes {
        return Err("full private writes exceeded the cgroup limit without an OOM".into());
    }
    Ok(Outcome::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::density::Step;

    fn fixture() -> (Case, Exit, Report) {
        let case = Case {
            memory_mib: 768,
            shape: Shape::Dirty,
            mode: "growth".into(),
            cow: true,
            dirty_mib: 1,
            guest_memory_mib: None,
            initial_cap_mib: None,
            repeat: 1,
        };
        let config = Config {
            shape: Shape::Dirty,
            cow: true,
            initial_bytes: MIB,
            memories: 1,
            swap_limit: 0,
            guest_limit: 576 * MIB,
            initial_limit: 2304 * MIB,
            process_limit: 691 * MIB,
            architecture: "test".into(),
            kernel: None,
            transparent_hugepage: None,
            outcome: None,
        };
        let steps = [Phase::Baseline, Phase::Ready, Phase::Writing]
            .into_iter()
            .map(|phase| Step {
                workloads: 2304,
                guest_memory_bytes: Some(0),
                initial_memory_bytes: Some(2304 * MIB),
                pressure: Some(Sample {
                    phase,
                    cgroup_current: None,
                    cgroup_peak: None,
                    guest_refused: 0,
                    initial_refused: 1,
                    process_refused: 0,
                    process_over: false,
                }),
                ..Default::default()
            })
            .collect();
        let report = Report {
            memory_limit_bytes: 768 * MIB,
            guest_memory_mode: Some("growth".into()),
            pressure: Some(config),
            steps,
            ..Default::default()
        };
        (
            case,
            Exit {
                exit_code: 137,
                oom_killed: true,
            },
            report,
        )
    }

    fn complete(report: &mut Report, count: usize) {
        for step in &mut report.steps {
            step.workloads = count;
            step.initial_memory_bytes = Some(count as u64 * MIB);
        }
        let template = report.steps.last().cloned().unwrap_or_default();
        for phase in [Phase::Touched, Phase::Released] {
            let mut step = template.clone();
            if let Some(sample) = &mut step.pressure {
                sample.phase = phase;
            }
            step.clean = phase == Phase::Touched;
            if phase == Phase::Released {
                step.workloads = 0;
                step.guest_memory_bytes = Some(0);
                step.initial_memory_bytes = Some(0);
            }
            report.steps.push(step);
        }
    }

    fn success() -> Exit {
        Exit {
            exit_code: 0,
            oom_killed: false,
        }
    }

    #[test]
    fn verified_late_write_oom_is_expected() {
        let (case, exit, report) = fixture();
        assert_eq!(classify(&case, &exit, &report), Ok(Outcome::ExpectedOom));
    }

    #[test]
    fn exit_137_alone_is_not_oom_evidence() {
        let (case, mut exit, report) = fixture();
        exit.oom_killed = false;
        assert!(classify(&case, &exit, &report).is_err());
    }

    #[test]
    fn oom_before_writes_is_a_failure() {
        let (case, exit, mut report) = fixture();
        report.steps.pop();
        assert!(classify(&case, &exit, &report).is_err());
        report.steps.pop();
        assert!(classify(&case, &exit, &report).is_err());
    }

    #[test]
    fn enforce_oom_is_a_failure() {
        let (mut case, exit, mut report) = fixture();
        case.mode = "enforce".into();
        report.guest_memory_mode = Some("enforce".into());
        for step in &mut report.steps {
            step.workloads = 576;
            step.guest_memory_bytes = Some(576 * MIB);
            step.initial_memory_bytes = Some(0);
        }
        assert!(classify(&case, &exit, &report).is_err());
    }

    #[test]
    fn sparse_oom_is_a_failure() {
        let (mut case, exit, mut report) = fixture();
        case.shape = Shape::Sparse;
        if let Some(config) = &mut report.pressure {
            config.shape = Shape::Sparse;
        }
        assert!(classify(&case, &exit, &report).is_err());
    }

    #[test]
    fn reported_limits_and_cow_must_match() {
        let (case, exit, report) = fixture();
        for setting in 0..4 {
            let mut report = report.clone();
            if let Some(config) = &mut report.pressure {
                match setting {
                    0 => config.cow = false,
                    1 => config.swap_limit = MIB,
                    2 => config.initial_limit = 2048 * MIB,
                    _ => report.memory_limit_bytes = 384 * MIB,
                }
            }
            assert!(classify(&case, &exit, &report).is_err());
        }
    }

    #[test]
    fn bounded_completion_preserves_admission_count_after_release() {
        let (case, _, mut report) = fixture();
        complete(&mut report, 576);
        assert_eq!(classify(&case, &success(), &report), Ok(Outcome::Completed));
        assert_eq!(report.achieved(), 576);
        assert_eq!(report.served(), 576);
        if let Some(step) = report.steps.last_mut() {
            step.initial_memory_bytes = Some(MIB);
        }
        assert!(classify(&case, &success(), &report).is_err());
    }

    #[test]
    fn missing_final_sample_is_a_failure() {
        let (case, _, mut report) = fixture();
        complete(&mut report, 576);
        report.steps.pop();
        assert!(classify(&case, &success(), &report).is_err());
    }

    #[test]
    fn unbounded_full_write_completion_is_invalid() {
        let (case, _, mut report) = fixture();
        complete(&mut report, 2304);
        assert!(classify(&case, &success(), &report).is_err());
    }

    #[test]
    fn sparse_completion_can_exceed_declared_container_memory() {
        let (mut case, _, mut report) = fixture();
        case.shape = Shape::Sparse;
        if let Some(config) = &mut report.pressure {
            config.shape = Shape::Sparse;
        }
        complete(&mut report, 2304);
        assert_eq!(classify(&case, &success(), &report), Ok(Outcome::Completed));
    }

    #[test]
    fn data_heavy_reservations_include_charged_initial_bytes() {
        let (mut case, _, mut report) = fixture();
        case.shape = Shape::DataHeavy;
        if let Some(config) = &mut report.pressure {
            config.shape = Shape::DataHeavy;
            config.initial_bytes = 8 * MIB;
        }
        complete(&mut report, 96);
        for step in report.steps.iter_mut().take(4) {
            step.guest_memory_bytes = Some(576 * MIB);
            step.initial_memory_bytes = Some(192 * MIB);
        }
        assert_eq!(classify(&case, &success(), &report), Ok(Outcome::Completed));
    }

    #[test]
    fn custom_budgets_complete() {
        let (mut case, _, mut report) = fixture();
        case.guest_memory_mib = Some(256);
        case.initial_cap_mib = Some(288);
        if let Some(config) = &mut report.pressure {
            config.guest_limit = 256 * MIB;
            config.initial_limit = 288 * MIB;
        }
        complete(&mut report, 288);
        assert_eq!(classify(&case, &success(), &report), Ok(Outcome::Completed));
    }

    #[test]
    fn incorrect_accounting_is_a_failure() {
        let (case, exit, mut report) = fixture();
        for step in &mut report.steps {
            step.initial_memory_bytes = Some(2303 * MIB);
        }
        assert!(classify(&case, &exit, &report).is_err());
    }

    #[test]
    fn over_budget_reservations_are_a_failure() {
        let (case, exit, mut report) = fixture();
        for step in &mut report.steps {
            step.workloads = 2305;
            step.initial_memory_bytes = Some(2305 * MIB);
        }
        assert!(classify(&case, &exit, &report).is_err());
    }

    #[test]
    fn unrelated_instantiation_failure_is_not_pressure() {
        let (case, exit, mut report) = fixture();
        for step in &mut report.steps {
            if let Some(sample) = &mut step.pressure {
                sample.initial_refused = 0;
            }
        }
        assert!(classify(&case, &exit, &report).is_err());
    }

    #[test]
    fn pressure_report_round_trips_through_shared_schema() -> anyhow::Result<()> {
        let (_, _, report) = fixture();
        let restored: Report = serde_json::from_slice(&serde_json::to_vec(&report)?)?;
        assert_eq!(restored.achieved(), report.achieved());
        assert_eq!(restored.pressure.map(|p| p.shape), Some(Shape::Dirty));
        Ok(())
    }

    #[test]
    fn oom_metrics_do_not_publish_an_unmeasured_final_peak_or_throughput() {
        let (_, _, mut report) = fixture();
        if let Some(config) = &mut report.pressure {
            config.outcome = Some(Outcome::ExpectedOom);
        }
        for step in &mut report.steps {
            if let Some(sample) = &mut step.pressure {
                sample.cgroup_peak = Some(100 * MIB);
            }
        }
        let metrics = report.metrics();
        assert!(!metrics.iter().any(|m| matches!(
            m.name,
            "cgroup_peak_bytes" | "starts_per_sec" | "request_p99_ms"
        )));
        assert!(
            metrics
                .iter()
                .any(|m| m.name == "oom_killed" && m.value == 1.0)
        );
        assert!(
            metrics
                .iter()
                .any(|m| m.name == "admitted_stores" && m.value == 2304.0)
        );
    }

    #[test]
    fn completed_metrics_include_retained_memory_after_cleanup() {
        let (_, _, mut report) = fixture();
        complete(&mut report, 576);
        if let Some(config) = &mut report.pressure {
            config.outcome = Some(Outcome::Completed);
        }
        let metrics = report.metrics();
        assert!(
            metrics
                .iter()
                .any(|m| m.name == "initial_memory_bytes" && m.value == (576 * MIB) as f64)
        );
        assert!(
            metrics
                .iter()
                .any(|m| m.name == "completed_stores" && m.value == 576.0)
        );
    }
}
