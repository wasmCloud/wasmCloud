//! Parse a `scripts/k6bench` result directory and derive the metrics the
//! report, JSONL and delta subcommands share.
//!
//! A result directory holds:
//!
//! - `summary.json`: k6's `handleSummary` data wrapped by
//!   `scripts/k6bench/lib/summary.js` with each scenario's offered rate and
//!   length. Only per-scenario submetrics (`http_reqs{scenario:measure}` …)
//!   are read, so the warm-up never counts.
//! - `metadata.json`: what `run.sh` ran and against which images.
//! - `cluster.ndjson`: ~1 s `docker stats` samples of the kind nodes and a
//!   dockerized k6 (absent for `--target kube`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

/// A run's measured window is valid only below these; see `derive`.
const MAX_ERROR_RATE: f64 = 0.01;
/// Share of its pinned core above which k6 itself is the bottleneck.
const GENERATOR_CPU_CEILING: f64 = 0.9;
/// Share of offered iterations k6 may drop before a window counts as not
/// sustained. A handful of drops in 100k is a scheduling blip, not a ceiling.
const MAX_DROPPED_RATIO: f64 = 0.001;

#[derive(Debug, Deserialize)]
pub struct Summary {
    pub scenario: String,
    pub profile: String,
    pub slo_p99_ms: f64,
    pub scenarios: BTreeMap<String, ScenarioShape>,
    pub k6: K6Data,
}

#[derive(Debug, Deserialize)]
pub struct ScenarioShape {
    pub rate: f64,
    pub duration_s: f64,
}

#[derive(Debug, Deserialize)]
pub struct K6Data {
    pub metrics: BTreeMap<String, K6Metric>,
}

#[derive(Debug, Deserialize)]
pub struct K6Metric {
    #[serde(default)]
    pub values: BTreeMap<String, f64>,
    #[serde(default)]
    pub thresholds: BTreeMap<String, ThresholdResult>,
}

#[derive(Debug, Deserialize)]
pub struct ThresholdResult {
    pub ok: bool,
}

/// `metadata.json` from run.sh. Everything optional past the identity, so an
/// older or hand-written file still renders.
#[derive(Debug, Default, Deserialize)]
pub struct RunMeta {
    #[serde(default)]
    pub rate: f64,
    #[serde(default)]
    pub warmup: String,
    #[serde(default)]
    pub workloads: u64,
    #[serde(default)]
    pub k6_mode: String,
    #[serde(default)]
    pub k6_version: String,
    #[serde(default)]
    pub k6_started: u64,
    #[serde(default)]
    pub k6_ended: u64,
    #[serde(default)]
    pub pinned: bool,
    /// CPU a native k6 used over its run, in cores. A dockerized k6 is
    /// sampled into `cluster.ndjson` instead.
    #[serde(default)]
    pub k6_cpu_cores: Option<f64>,
    #[serde(default)]
    pub wash_image: String,
    #[serde(default)]
    pub deploy_ready_s: Option<f64>,
    #[serde(default)]
    pub target: String,
}

#[derive(Debug, Deserialize)]
struct RawSample {
    ts: u64,
    name: String,
    cpu: String,
    mem: String,
}

/// One `docker stats` sample: CPU in cores (100 % = 1 core), memory in MiB.
#[derive(Debug, Clone)]
pub struct Sample {
    pub ts: u64,
    pub name: String,
    pub cpu_cores: f64,
    pub mem_mib: f64,
}

/// Per-scenario numbers read from k6's `{scenario:<name>}` submetrics.
#[derive(Debug, Clone)]
pub struct ScenarioStats {
    pub name: String,
    pub offered_rps: f64,
    pub requests: f64,
    pub rps: f64,
    pub error_rate: f64,
    pub dropped: f64,
    pub offered: f64,
    /// `p50`, `p90`, `p95`, `p99`, `p99.9`, `avg`, `max`, all in ms.
    pub latency_ms: BTreeMap<&'static str, f64>,
}

impl ScenarioStats {
    pub fn p99(&self) -> f64 {
        self.latency_ms.get("p99").copied().unwrap_or(f64::NAN)
    }

    /// k6 couldn't start more than a blip's worth of the offered iterations.
    pub fn dropped_too_many(&self) -> bool {
        self.offered > 0.0 && self.dropped / self.offered > MAX_DROPPED_RATIO
    }

    /// Held the SLO: under 1 % errors, p99 within the ceiling, and k6 kept up.
    pub fn sustained(&self, slo_p99_ms: f64) -> bool {
        self.error_rate < MAX_ERROR_RATE && self.p99() <= slo_p99_ms && !self.dropped_too_many()
    }
}

/// Which way is good for a metric; drives the delta arrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Better {
    Higher,
    Lower,
}

impl Better {
    pub fn as_str(self) -> &'static str {
        match self {
            Better::Higher => "higher",
            Better::Lower => "lower",
        }
    }
}

/// A headline number: one history.json row, one delta line.
#[derive(Debug, Clone)]
pub struct Metric {
    pub name: String,
    pub value: f64,
    pub unit: &'static str,
    pub better: Better,
}

pub struct Run {
    pub dir: PathBuf,
    pub summary: Summary,
    pub meta: RunMeta,
    pub samples: Vec<Sample>,
    pub stats: Vec<ScenarioStats>,
}

impl Run {
    pub fn load(dir: &Path) -> Result<Self> {
        let summary: Summary = read_json(&dir.join("summary.json"))?;
        let meta_path = dir.join("metadata.json");
        let meta: RunMeta = if meta_path.exists() {
            read_json(&meta_path)?
        } else {
            RunMeta::default()
        };
        let samples = read_samples(&dir.join("cluster.ndjson"))?;
        let stats = summary
            .scenarios
            .iter()
            .filter(|(name, _)| name.as_str() != "warmup")
            .map(|(name, shape)| scenario_stats(&summary.k6, name, shape))
            .collect();
        Ok(Self {
            dir: dir.to_path_buf(),
            summary,
            meta,
            samples,
            stats,
        })
    }

    /// The `param` of the history row: the profile, plus the offered rate for
    /// the fixed-rate profiles and the workload count for many-workloads,
    /// since numbers at a different rate or count don't compare.
    pub fn param(&self) -> String {
        let mut param = match self.summary.profile.as_str() {
            "stress" => "stress".to_string(),
            p => format!("{p}-{}", self.meta.rate.max(self.offered_base_rate())),
        };
        if self.summary.scenario == "many-workloads" {
            param.push_str(&format!("-w{}", self.meta.workloads));
        }
        param
    }

    fn offered_base_rate(&self) -> f64 {
        self.summary
            .scenarios
            .get("measure")
            .or_else(|| self.summary.scenarios.get("spike_pre"))
            .map_or(0.0, |s| s.rate)
    }

    pub fn scenario(&self, name: &str) -> Option<&ScenarioStats> {
        self.stats.iter().find(|s| s.name == name)
    }

    /// Stress steps in the order they ran (by offered rate).
    pub fn steps(&self) -> Vec<&ScenarioStats> {
        let mut steps: Vec<&ScenarioStats> = self
            .stats
            .iter()
            .filter(|s| s.name.starts_with("step_"))
            .collect();
        steps.sort_by(|a, b| a.offered_rps.total_cmp(&b.offered_rps));
        steps
    }

    /// Every threshold k6 evaluated held. A breach is a result, not an error.
    pub fn thresholds_passed(&self) -> bool {
        self.summary
            .k6
            .metrics
            .values()
            .flat_map(|m| m.thresholds.values())
            .all(|t| t.ok)
    }

    /// k6, not wasmCloud, set the ceiling: its pinned core was saturated. Such
    /// a run is reported but not published.
    ///
    /// Dropped iterations alone don't say so: k6 also drops them when a slow
    /// system ties up every VU, and that is wasmCloud's result to publish
    /// (`sustained` already fails the SLO for it). Unpinned, k6 can use any
    /// core, so there is no ceiling to check.
    pub fn generator_saturated(&self) -> bool {
        let k6_cpu = self.meta.k6_cpu_cores.or_else(|| {
            self.cluster_avg(|name| name.ends_with("-k6"))
                .map(|(cpu, _)| cpu)
        });
        self.meta.pinned && k6_cpu.is_some_and(|c| c > GENERATOR_CPU_CEILING)
    }

    /// Mean CPU (cores) and peak memory (MiB) of the matching containers over
    /// the measured window (after warm-up, until k6 ended).
    pub fn cluster_avg(&self, pick: impl Fn(&str) -> bool) -> Option<(f64, f64)> {
        let start = self.meta.k6_started + parse_seconds(&self.meta.warmup).unwrap_or(0);
        let end = if self.meta.k6_ended == 0 {
            u64::MAX
        } else {
            self.meta.k6_ended
        };
        let window: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|s| pick(&s.name) && s.ts >= start && s.ts <= end)
            .collect();
        if window.is_empty() {
            return None;
        }
        let cpu = window.iter().map(|s| s.cpu_cores).sum::<f64>() / window.len() as f64;
        let mem = window.iter().map(|s| s.mem_mib).fold(0.0, f64::max);
        Some((cpu, mem))
    }

    /// The headline numbers for this run's profile, in display order.
    pub fn metrics(&self) -> Vec<Metric> {
        let mut out = Vec::new();
        let mut push = |name: &str, value: f64, unit: &'static str, better: Better| {
            if value.is_finite() {
                out.push(Metric {
                    name: name.to_string(),
                    value,
                    unit,
                    better,
                });
            }
        };

        match self.summary.profile.as_str() {
            "stress" => {
                let (max, knee) = self.max_sustainable();
                push("max_sustainable_rps", max, "rps", Better::Higher);
                if let Some(knee) = knee {
                    push("knee_offered_rps", knee, "rps", Better::Higher);
                }
            }
            "spike" => {
                for (scenario, label) in [("spike_peak", "peak"), ("spike_post", "post")] {
                    if let Some(s) = self.scenario(scenario) {
                        push(&format!("{label}_rps"), s.rps, "rps", Better::Higher);
                        push(&format!("{label}_p99_ms"), s.p99(), "ms", Better::Lower);
                        push(
                            &format!("{label}_error_rate"),
                            s.error_rate,
                            "ratio",
                            Better::Lower,
                        );
                    }
                }
            }
            _ => {
                if let Some(s) = self.scenario("measure") {
                    push("rps", s.rps, "rps", Better::Higher);
                    for (key, name) in [
                        ("p50", "p50_ms"),
                        ("p90", "p90_ms"),
                        ("p95", "p95_ms"),
                        ("p99", "p99_ms"),
                        ("p99.9", "p999_ms"),
                    ] {
                        if let Some(v) = s.latency_ms.get(key) {
                            push(name, *v, "ms", Better::Lower);
                        }
                    }
                    push("error_rate", s.error_rate, "ratio", Better::Lower);
                }
            }
        }

        if let Some((cpu, mem)) = self.cluster_avg(|n| n.ends_with("-worker")) {
            push("host_node_cpu_cores_avg", cpu, "cores", Better::Lower);
            push("host_node_mem_mib_peak", mem, "MiB", Better::Lower);
        }
        if let Some(ready) = self.meta.deploy_ready_s {
            push("deploy_ready_s", ready, "s", Better::Lower);
        }
        out
    }

    /// Highest rps achieved on a step that held the SLO, walking up until the
    /// first step that didn't (the knee, returned as its offered rate).
    fn max_sustainable(&self) -> (f64, Option<f64>) {
        let mut best = f64::NAN;
        for step in self.steps() {
            if !step.sustained(self.summary.slo_p99_ms) {
                return (best, Some(step.offered_rps));
            }
            best = if best.is_nan() {
                step.rps
            } else {
                best.max(step.rps)
            };
        }
        (best, None)
    }
}

fn scenario_stats(k6: &K6Data, name: &str, shape: &ScenarioShape) -> ScenarioStats {
    let metric = |base: &str| k6.metrics.get(&format!("{base}{{scenario:{name}}}"));
    let value = |base: &str, key: &str| {
        metric(base)
            .and_then(|m| m.values.get(key))
            .copied()
            .unwrap_or(0.0)
    };

    let requests = value("http_reqs", "count");
    let mut latency_ms = BTreeMap::new();
    if let Some(m) = metric("http_req_duration") {
        for (k6_key, key) in [
            ("avg", "avg"),
            ("p(50)", "p50"),
            ("p(90)", "p90"),
            ("p(95)", "p95"),
            ("p(99)", "p99"),
            ("p(99.9)", "p99.9"),
            ("max", "max"),
        ] {
            if let Some(v) = m.values.get(k6_key) {
                latency_ms.insert(key, *v);
            }
        }
    }

    ScenarioStats {
        name: name.to_string(),
        offered_rps: shape.rate,
        requests,
        rps: if shape.duration_s > 0.0 {
            requests / shape.duration_s
        } else {
            f64::NAN
        },
        error_rate: value("http_req_failed", "rate"),
        dropped: value("dropped_iterations", "count"),
        offered: shape.rate * shape.duration_s,
        latency_ms,
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn read_samples(path: &Path) -> Result<Vec<Sample>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut samples = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        // A sample cut short by the sampler being killed is skipped, not fatal.
        let Ok(raw) = serde_json::from_str::<RawSample>(line) else {
            continue;
        };
        let (Some(cpu), Some(mem)) = (parse_percent(&raw.cpu), parse_mem_mib(&raw.mem)) else {
            continue;
        };
        samples.push(Sample {
            ts: raw.ts,
            name: raw.name,
            cpu_cores: cpu / 100.0,
            mem_mib: mem,
        });
    }
    Ok(samples)
}

fn parse_percent(s: &str) -> Option<f64> {
    s.trim().trim_end_matches('%').parse().ok()
}

/// The used half of `docker stats`' `MemUsage`, e.g. `1.2GiB / 7.6GiB`.
fn parse_mem_mib(s: &str) -> Option<f64> {
    let used = s.split('/').next()?.trim();
    let split = used.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = used.split_at(split);
    let num: f64 = num.trim().parse().ok()?;
    let mib = match unit {
        "B" => num / (1024.0 * 1024.0),
        "KiB" => num / 1024.0,
        "kB" => num * 1000.0 / (1024.0 * 1024.0),
        "MiB" => num,
        "MB" => num * 1_000_000.0 / (1024.0 * 1024.0),
        "GiB" => num * 1024.0,
        "GB" => num * 1_000_000_000.0 / (1024.0 * 1024.0),
        _ => return None,
    };
    Some(mib)
}

/// `30s`, `2m`, `1h`, or bare seconds (the same grammar as lib/config.js).
pub fn parse_seconds(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('s') => (&s[..s.len() - 1], 1),
        Some('m') => (&s[..s.len() - 1], 60),
        Some('h') => (&s[..s.len() - 1], 3600),
        _ => (s, 1),
    };
    num.parse::<u64>()
        .map(|n| n * mult)
        .map_err(|_| anyhow!("unparseable duration {s:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/k6")
            .join(name)
    }

    #[test]
    fn parses_docker_stats_memory() {
        assert_eq!(parse_mem_mib("512MiB / 7.6GiB"), Some(512.0));
        assert_eq!(parse_mem_mib("1.5GiB / 7.6GiB"), Some(1536.0));
        assert_eq!(parse_mem_mib("?"), None);
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_seconds("30s").ok(), Some(30));
        assert_eq!(parse_seconds("2m").ok(), Some(120));
        assert_eq!(parse_seconds("45").ok(), Some(45));
        assert!(parse_seconds("soon").is_err());
    }

    #[test]
    fn constant_run_reads_only_the_measured_window() -> Result<()> {
        let run = Run::load(&fixture("constant"))?;
        let measure = run
            .scenario("measure")
            .ok_or_else(|| anyhow!("no measure scenario"))?;
        // The warm-up's requests are in http_reqs but not in the submetric.
        let total = run.summary.k6.metrics["http_reqs"].values["count"];
        assert!(measure.requests < total);
        assert!(run.metrics().iter().any(|m| m.name == "p99_ms"));
        assert!(!run.generator_saturated());
        Ok(())
    }

    #[test]
    fn only_k6_cpu_marks_a_run_generator_saturated() -> Result<()> {
        let mut run = Run::load(&fixture("constant"))?;
        run.meta.pinned = true;
        run.meta.k6_cpu_cores = Some(0.95);
        assert!(run.generator_saturated(), "a pinned k6 at 95% of its core");

        // A slow system makes k6 drop iterations too; that is a failed SLO
        // to publish, not a saturated generator.
        run.meta.k6_cpu_cores = Some(0.2);
        if let Some(measure) = run.stats.iter_mut().find(|s| s.name == "measure") {
            measure.dropped = measure.offered;
        }
        assert!(!run.generator_saturated());
        assert!(run.scenario("measure").is_some_and(|m| !m.sustained(250.0)));

        run.meta.pinned = false;
        run.meta.k6_cpu_cores = Some(3.0);
        assert!(!run.generator_saturated(), "unpinned k6 has no ceiling");
        Ok(())
    }

    #[test]
    fn stress_run_finds_the_knee() -> Result<()> {
        let mut run = Run::load(&fixture("stress"))?;
        let find = |run: &Run, name: &str| {
            run.metrics()
                .into_iter()
                .find(|m| m.name == name)
                .map(|m| m.value)
        };

        // Every step holds a 20 ms p99, including step_4000 whose 7 drops
        // in 120k are under the blip tolerance: no knee, top step wins.
        run.summary.slo_p99_ms = 20.0;
        assert_eq!(find(&run, "knee_offered_rps"), None);
        let top = run.steps().last().map(|s| s.rps);
        assert_eq!(find(&run, "max_sustainable_rps"), top);

        // The worst p99 is step_500's 9.33 ms: 9.5 ms still holds everywhere,
        // 9.0 ms breaks the first step, so nothing was sustained.
        run.summary.slo_p99_ms = 9.5;
        assert_eq!(find(&run, "knee_offered_rps"), None);
        run.summary.slo_p99_ms = 9.0;
        assert_eq!(find(&run, "knee_offered_rps"), Some(500.0));
        assert_eq!(find(&run, "max_sustainable_rps"), None);
        Ok(())
    }
}
