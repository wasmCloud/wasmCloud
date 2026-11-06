//! Container memory pressure runs: the case a runner asks for,
//! the samples the `memory_pressure` bench records, and the outcome checks.
//!
//! The bench checks guest accounting against the runtime's own constants
//! before it writes each later sample, so a sample's presence is evidence.
//! [`classify`] only decides what the exit and the samples mean together.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::density::{Better, Metric, render_bytes};

pub const MIB: u64 = 1 << 20;

/// Carries the JSON [`Case`] into the bench.
pub const CASE_ENV: &str = "WASH_PRESSURE_CASE";
/// Names the file the bench writes its [`Report`] to.
pub const REPORT_ENV: &str = "WASH_PRESSURE_REPORT";
/// Container memory limits a case may request, in MiB.
pub const MEMORY_MIB: RangeInclusive<u64> = 384..=2048;
/// Initial memory sizes the `dirty` shape may request, in MiB.
pub const DIRTY_MIB: RangeInclusive<u64> = 1..=2;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Enforce,
    Growth,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::Growth => "growth",
        }
    }
}

/// One container run, as the runner requests it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    pub memory_mib: u64,
    pub shape: Shape,
    pub mode: Mode,
    pub cow: bool,
    pub dirty_mib: u64,
    pub guest_memory_mib: Option<u64>,
    pub initial_cap_mib: Option<u64>,
    pub repeat: u32,
}

impl Case {
    pub fn label(&self) -> String {
        format!(
            "{}m-{}-{}-cow-{}-{}",
            self.memory_mib,
            self.shape.name(),
            self.mode.name(),
            if self.cow { "on" } else { "off" },
            self.repeat
        )
    }

    /// Memories, initial bytes per memory, and data-segment bytes per memory.
    pub fn dimensions(&self) -> (u64, u64, u64) {
        match self.shape {
            Shape::Sparse => (1, MIB, 4096),
            Shape::DataHeavy => (1, 8 * MIB, 8 * MIB),
            Shape::Multiple => (4, MIB, 4096),
            Shape::Dirty => (1, self.dirty_mib * MIB, 4096),
        }
    }

    /// Declared initial bytes across one store's memories.
    pub fn initial_bytes(&self) -> u64 {
        let (memories, per_memory, _) = self.dimensions();
        memories * per_memory
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Engine built, nothing admitted.
    Baseline,
    /// Admission stopped; every admitted store is alive. Writes start next.
    Ready,
    /// Every store finished its writes and is still alive.
    Touched,
    /// Every store dropped and both reservation counters back at zero.
    Released,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Ready => "ready",
            Self::Touched => "touched",
            Self::Released => "released",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub phase: Phase,
    pub stores: usize,
    pub footprint_bytes: Option<u64>,
    /// Charged guest memory.
    pub guest_memory_bytes: u64,
    /// Uncharged initial guest memory.
    pub initial_memory_bytes: u64,
    pub cgroup_current: Option<u64>,
    pub cgroup_peak: Option<u64>,
    /// Refusal counters, cumulative since the engine was built.
    pub guest_refused: u64,
    pub initial_refused: u64,
    pub process_refused: u64,
    pub process_over: bool,
}

impl Sample {
    pub fn refused(&self) -> u64 {
        self.guest_refused + self.initial_refused + self.process_refused
    }
}

/// What the bench saw of its container and engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub architecture: String,
    pub kernel: Option<String>,
    /// `transparent_hugepage/enabled`, which governs anonymous memories (CoW off).
    pub transparent_hugepage: Option<String>,
    /// `transparent_hugepage/shmem_enabled`, which governs CoW image mappings.
    pub shmem_hugepage: Option<String>,
    /// CPUs from `cpu.max`; `None` when unlimited.
    pub cpus: Option<f64>,
    pub memory_limit: u64,
    pub guest_limit: u64,
    pub initial_limit: u64,
    pub process_limit: u64,
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

/// The record one case leaves; `<target>/pressure/<bench>.json` holds a list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub case: Case,
    /// `None` when the bench exited before describing its host.
    pub host: Option<Host>,
    pub samples: Vec<Sample>,
    /// Why admission stopped, then why the case failed, if it did.
    pub stop: String,
    /// Set by the runner after [`classify`].
    pub outcome: Option<Outcome>,
}

impl Report {
    pub fn new(case: Case) -> Self {
        Self {
            case,
            host: None,
            samples: Vec::new(),
            stop: String::new(),
            outcome: None,
        }
    }

    pub fn sample(&self, phase: Phase) -> Option<&Sample> {
        self.samples.iter().find(|s| s.phase == phase)
    }

    /// Stores alive when admission stopped.
    pub fn admitted(&self) -> usize {
        self.sample(Phase::Ready).map_or(0, |s| s.stores)
    }

    /// Stores that finished their writes.
    pub fn completed(&self) -> usize {
        self.sample(Phase::Touched).map_or(0, |s| s.stores)
    }

    /// The highest cgroup peak, only when the run completed: an OOM case's
    /// last reading is not its final peak.
    pub fn cgroup_peak(&self) -> Option<u64> {
        (self.outcome == Some(Outcome::Completed))
            .then(|| self.samples.iter().filter_map(|s| s.cgroup_peak).max())
            .flatten()
    }

    /// The figures `bench-tools jsonl` emits. Unmeasured figures are left out.
    pub fn metrics(&self) -> Vec<Metric> {
        let ready = self.sample(Phase::Ready);
        [
            (
                "admitted_stores",
                ready.map(|s| s.stores as f64),
                "stores",
                Better::Higher,
            ),
            (
                "completed_stores",
                self.sample(Phase::Touched).map(|s| s.stores as f64),
                "stores",
                Better::Higher,
            ),
            (
                "guest_memory_bytes",
                ready.map(|s| s.guest_memory_bytes as f64),
                "B",
                Better::Lower,
            ),
            (
                "initial_memory_bytes",
                ready.map(|s| s.initial_memory_bytes as f64),
                "B",
                Better::Lower,
            ),
            (
                "cgroup_peak_bytes",
                self.cgroup_peak().map(|n| n as f64),
                "B",
                Better::Lower,
            ),
            (
                "oom_killed",
                match self.outcome {
                    Some(Outcome::Completed) => Some(0.0),
                    Some(Outcome::ExpectedOom) => Some(1.0),
                    Some(Outcome::Failed) | None => None,
                },
                "bool",
                Better::Lower,
            ),
        ]
        .into_iter()
        .filter_map(|(name, value, unit, better)| {
            Some(Metric {
                name,
                value: value?,
                unit,
                better,
            })
        })
        .collect()
    }
}

/// Docker's view of how the container exited.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Exit {
    pub exit_code: i32,
    #[serde(rename = "OOMKilled")]
    pub oom_killed: bool,
}

/// An expected OOM needs Docker's OOM flag, a `growth` `dirty` case,
/// and a kill after admission completed but before writes finished.
pub fn classify(report: &Report, exit: &Exit) -> Result<Outcome, String> {
    let case = &report.case;
    let host = report.host.as_ref().ok_or("no report before exit")?;
    let ready = report
        .sample(Phase::Ready)
        .ok_or("failed before admission completed")?;
    if ready.stores == 0 {
        return Err("no guests admitted".into());
    }
    if !ready.process_over && ready.refused() == 0 {
        return Err("admission stopped without memory-pressure evidence".into());
    }
    let phases: Vec<Phase> = report.samples.iter().map(|s| s.phase).collect();
    if exit.oom_killed {
        if case.mode == Mode::Growth
            && case.shape == Shape::Dirty
            && exit.exit_code == 137
            && phases == [Phase::Baseline, Phase::Ready]
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
            Phase::Baseline,
            Phase::Ready,
            Phase::Touched,
            Phase::Released,
        ]
    {
        return Err("missing or out-of-order completion samples".into());
    }
    if case.shape == Shape::Dirty && ready.stores as u64 * case.initial_bytes() > host.memory_limit
    {
        return Err("full private writes exceeded the cgroup limit without an OOM".into());
    }
    Ok(Outcome::Completed)
}

/// One row of the live sample table, on stderr; `header` first.
pub fn print_sample(header: bool, sample: &Sample) {
    if header {
        eprintln!(
            "{:>10} {:>9} {:>10} {:>10} {:>10} {:>10} {:>10}",
            "phase", "stores", "footprint", "charged", "initial", "cgroup", "peak"
        );
    }
    eprintln!(
        "{:>10} {:>9} {:>10} {:>10} {:>10} {:>10} {:>10}",
        sample.phase.name(),
        sample.stores,
        or_dash(sample.footprint_bytes),
        render_bytes(sample.guest_memory_bytes),
        render_bytes(sample.initial_memory_bytes),
        or_dash(sample.cgroup_current),
        or_dash(sample.cgroup_peak),
    );
}

/// One line per case, on stderr.
pub fn print_summary(reports: &[Report]) {
    eprintln!(
        "{:<40} {:>24} {:>9} {:>9} {:>10} {:>10} {:>10}",
        "case", "outcome", "admitted", "completed", "charged", "initial", "peak"
    );
    for report in reports {
        let ready = report.sample(Phase::Ready);
        eprintln!(
            "{:<40} {:>24} {:>9} {:>9} {:>10} {:>10} {:>10}",
            report.case.label(),
            report.outcome.map_or("-", Outcome::name),
            report.admitted(),
            report.completed(),
            or_dash(ready.map(|s| s.guest_memory_bytes)),
            or_dash(ready.map(|s| s.initial_memory_bytes)),
            or_dash(report.cgroup_peak()),
        );
    }
}

fn or_dash(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "-".into(), render_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(phase: Phase, stores: usize) -> Sample {
        Sample {
            phase,
            stores,
            footprint_bytes: None,
            guest_memory_bytes: 0,
            initial_memory_bytes: stores as u64 * MIB,
            cgroup_current: None,
            cgroup_peak: Some(100 * MIB),
            guest_refused: 0,
            initial_refused: 1,
            process_refused: 0,
            process_over: false,
        }
    }

    /// A growth `dirty` case killed during its writes.
    fn oom() -> (Report, Exit) {
        let mut report = Report::new(Case {
            memory_mib: 768,
            shape: Shape::Dirty,
            mode: Mode::Growth,
            cow: true,
            dirty_mib: 1,
            guest_memory_mib: None,
            initial_cap_mib: None,
            repeat: 1,
        });
        report.host = Some(Host {
            architecture: "test".into(),
            kernel: None,
            transparent_hugepage: None,
            shmem_hugepage: None,
            cpus: Some(2.0),
            memory_limit: 768 * MIB,
            guest_limit: 576 * MIB,
            initial_limit: 2304 * MIB,
            process_limit: 691 * MIB,
        });
        report.samples = vec![sample(Phase::Baseline, 0), sample(Phase::Ready, 2304)];
        let exit = Exit {
            exit_code: 137,
            oom_killed: true,
        };
        (report, exit)
    }

    /// The same case with `stores` admitted, run to completion.
    fn completed(stores: usize) -> (Report, Exit) {
        let (mut report, _) = oom();
        report.samples = vec![
            sample(Phase::Baseline, 0),
            sample(Phase::Ready, stores),
            sample(Phase::Touched, stores),
            sample(Phase::Released, 0),
        ];
        let exit = Exit {
            exit_code: 0,
            oom_killed: false,
        };
        (report, exit)
    }

    #[test]
    fn verified_late_write_oom_is_expected() {
        let (report, exit) = oom();
        assert_eq!(classify(&report, &exit), Ok(Outcome::ExpectedOom));
    }

    #[test]
    fn oom_near_the_limit_is_still_expected() {
        // Host overhead can push a just-fitting write set over the limit.
        let (mut report, exit) = oom();
        report.samples = vec![sample(Phase::Baseline, 0), sample(Phase::Ready, 700)];
        assert_eq!(classify(&report, &exit), Ok(Outcome::ExpectedOom));
    }

    #[test]
    fn exit_137_alone_is_not_oom_evidence() {
        let (report, mut exit) = oom();
        exit.oom_killed = false;
        assert!(classify(&report, &exit).is_err());
    }

    #[test]
    fn oom_before_admission_completed_is_a_failure() {
        let (mut report, exit) = oom();
        report.samples.pop();
        assert!(classify(&report, &exit).is_err());
    }

    #[test]
    fn enforce_or_sparse_oom_is_a_failure() {
        let (mut report, exit) = oom();
        report.case.mode = Mode::Enforce;
        assert!(classify(&report, &exit).is_err());
        let (mut report, exit) = oom();
        report.case.shape = Shape::Sparse;
        assert!(classify(&report, &exit).is_err());
    }

    #[test]
    fn missing_host_is_a_failure() {
        let (mut report, exit) = completed(576);
        report.host = None;
        assert!(classify(&report, &exit).is_err());
    }

    #[test]
    fn bounded_completion_passes() {
        let (report, exit) = completed(576);
        assert_eq!(classify(&report, &exit), Ok(Outcome::Completed));
    }

    #[test]
    fn missing_final_sample_is_a_failure() {
        let (mut report, exit) = completed(576);
        report.samples.pop();
        assert!(classify(&report, &exit).is_err());
    }

    #[test]
    fn full_writes_past_the_limit_cannot_complete() {
        let (report, exit) = completed(2304);
        assert!(classify(&report, &exit).is_err());
    }

    #[test]
    fn sparse_completion_can_exceed_declared_container_memory() {
        let (mut report, exit) = completed(2304);
        report.case.shape = Shape::Sparse;
        assert_eq!(classify(&report, &exit), Ok(Outcome::Completed));
    }

    #[test]
    fn admission_without_pressure_evidence_is_a_failure() {
        let (mut report, exit) = oom();
        for sample in &mut report.samples {
            sample.initial_refused = 0;
        }
        assert!(classify(&report, &exit).is_err());
    }

    #[test]
    fn report_round_trips() -> anyhow::Result<()> {
        let (report, _) = oom();
        let restored: Report = serde_json::from_slice(&serde_json::to_vec(&report)?)?;
        assert_eq!(restored.case, report.case);
        assert_eq!(restored.admitted(), 2304);
        Ok(())
    }

    #[test]
    fn oom_metrics_leave_out_the_unknown_final_peak() {
        let (mut report, _) = oom();
        report.outcome = Some(Outcome::ExpectedOom);
        let metrics = report.metrics();
        let value = |name| metrics.iter().find(|m| m.name == name).map(|m| m.value);
        assert_eq!(value("cgroup_peak_bytes"), None);
        assert_eq!(value("completed_stores"), None);
        assert_eq!(value("oom_killed"), Some(1.0));
        assert_eq!(value("admitted_stores"), Some(2304.0));
    }

    #[test]
    fn completed_metrics_include_peak_and_reservations() {
        let (mut report, _) = completed(576);
        report.outcome = Some(Outcome::Completed);
        let metrics = report.metrics();
        let value = |name| metrics.iter().find(|m| m.name == name).map(|m| m.value);
        assert_eq!(value("completed_stores"), Some(576.0));
        assert_eq!(value("initial_memory_bytes"), Some((576 * MIB) as f64));
        assert_eq!(value("cgroup_peak_bytes"), Some((100 * MIB) as f64));
        assert_eq!(value("oom_killed"), Some(0.0));
    }
}
