//! Reads a `scripts/gpubench/run.sh` result directory into history rows.
//!
//! - The gap bench: tok/s for each step (`vendor`, `webgpu-native`, `wasm`)
//!   on each model, and the gaps between them. Repetitions run interleaved
//!   across steps, one llama-bench run each, and are merged here.
//! - The serverless bench: per model, where a cold-started turn's time goes
//!   (start, model load, first token); and GPU memory while busy, while idle
//!   between turns, and before the first turn. Both for wasmCloud's
//!   workloads and for the same tool run as native processes.
//!
//! GPU memory is reported over the baseline sampled before the run starts,
//! so a box's other GPU users (the window server on macOS) drop out.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::k6::Better;

/// Steps of the gap bench, in the order they're compared.
pub const STEPS: [&str; 3] = ["vendor", "webgpu-native", "wasm"];

/// webgpu-native with every CPU thread, for the headline model only: if it
/// moves from the one-thread step, CPU work is part of the WebGPU number.
pub const THREADS_STEP: &str = "webgpu-native-mt";

/// The serverless bench's two sides.
pub const SIDES: [&str; 2] = ["wasmcloud", "native"];

/// Timeline points kept on the row that carries the memory-over-time chart.
const TIMELINE_POINTS: usize = 600;

#[derive(Debug, Deserialize)]
pub struct Metadata {
    pub box_type: String,
    pub gpu: String,
    /// The model the headline gap rows are about.
    #[serde(default)]
    pub gap_model: Option<String>,
    #[serde(default)]
    pub smoke: bool,
    #[serde(default)]
    pub wgpu: String,
    #[serde(default)]
    pub llama_cpp: String,
    /// Steps run.sh recorded as failed.
    #[serde(default)]
    pub failed: Vec<String>,
}

/// One entry of llama-bench's `-o json` output.
#[derive(Debug, Clone, Deserialize)]
pub struct LlamaTest {
    pub n_prompt: u64,
    pub n_gen: u64,
    pub avg_ts: f64,
    pub stddev_ts: f64,
    #[serde(default)]
    pub samples_ts: Vec<f64>,
    pub backends: String,
    pub model_size: u64,
    #[serde(default)]
    pub n_threads: u64,
}

impl LlamaTest {
    /// `pp512` or `tg128`, as llama-bench names its tests.
    pub fn name(&self) -> String {
        if self.n_prompt > 0 {
            format!("pp{}", self.n_prompt)
        } else {
            format!("tg{}", self.n_gen)
        }
    }

    /// Repetitions this result covers.
    pub fn reps(&self) -> usize {
        self.samples_ts.len().max(1)
    }
}

/// Merges one llama-bench run per repetition into one result per test, with
/// the mean and sample standard deviation over every repetition's samples.
pub fn merge_reps(runs: Vec<Vec<LlamaTest>>) -> Vec<LlamaTest> {
    let mut by_test: BTreeMap<String, LlamaTest> = BTreeMap::new();
    for t in runs.into_iter().flatten() {
        let samples = if t.samples_ts.is_empty() {
            vec![t.avg_ts]
        } else {
            t.samples_ts.clone()
        };
        by_test
            .entry(t.name())
            .and_modify(|m| m.samples_ts.extend(&samples))
            .or_insert(LlamaTest {
                samples_ts: samples,
                ..t
            });
    }
    by_test
        .into_values()
        .map(|mut t| {
            let n = t.samples_ts.len() as f64;
            let mean = t.samples_ts.iter().sum::<f64>() / n;
            let var = if t.samples_ts.len() > 1 {
                t.samples_ts.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)
            } else {
                0.0
            };
            t.avg_ts = mean;
            t.stddev_ts = var.sqrt();
            t
        })
        .collect()
}

/// bench-gpu's report for one `run`.
#[derive(Debug, Deserialize)]
pub struct WasmReport {
    pub exit_code: Option<i32>,
    pub guest_memory_peak: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GpuSample {
    pub t_ms: u64,
    pub used: u64,
}

/// One bench-gpu `turns` event.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready {
        t_ms: u64,
    },
    TurnStart {
        t_ms: u64,
        name: String,
    },
    TurnEnd {
        t_ms: u64,
        name: String,
        exit_code: Option<i32>,
        resolve_ms: Option<f64>,
        run_ms: f64,
        #[serde(default)]
        load_ms: Option<f64>,
        #[serde(default)]
        first_token_ms: Option<f64>,
    },
    Idle {
        t_ms: u64,
        until_ms: u64,
    },
    Stopped {
        t_ms: u64,
    },
}

/// One side of the serverless bench: its events and the GPU samples taken
/// while it ran.
#[derive(Debug)]
pub struct Serving {
    pub events: Vec<Event>,
    pub samples: Vec<GpuSample>,
}

/// The gap bench for one model.
#[derive(Debug, Default)]
pub struct Gap {
    /// llama-bench results per step; a step that didn't run is absent.
    pub steps: BTreeMap<String, Vec<LlamaTest>>,
    /// The largest guest memory any wasm repetition reached, and whether
    /// every one exited 0.
    pub wasm_report: Option<WasmReport>,
}

#[derive(Debug)]
pub struct GpuRun {
    pub meta: Metadata,
    /// Per model, by the name models.json gives it.
    pub gap: BTreeMap<String, Gap>,
    /// Per side of the serverless bench.
    pub serverless: BTreeMap<String, Serving>,
}

/// A history row before provenance is attached.
#[derive(Debug, Clone)]
pub struct GpuRow {
    pub group: String,
    pub param: String,
    pub metric: &'static str,
    pub value: f64,
    pub unit: &'static str,
    pub better: Better,
    pub extra: serde_json::Map<String, Value>,
}

impl GpuRun {
    pub fn load(dir: &Path) -> Result<Self> {
        let meta: Metadata = {
            let mut v: Value = read_json(&dir.join("metadata.json"))?;
            // `box` is a keyword; the file keeps the shell's name for it.
            if let Some(obj) = v.as_object_mut()
                && let Some(b) = obj.remove("box")
            {
                obj.insert("box_type".into(), b);
            }
            serde_json::from_value(v).context("metadata.json")?
        };
        let mut gap = BTreeMap::new();
        let gap_dir = dir.join("gap");
        if gap_dir.is_dir() {
            for entry in std::fs::read_dir(&gap_dir)? {
                let path = entry?.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(model) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let g = load_gap(&path)?;
                if !g.steps.is_empty() {
                    gap.insert(model.to_string(), g);
                }
            }
        }
        let mut serverless = BTreeMap::new();
        for side in SIDES {
            let events = dir.join("serverless").join(format!("{side}.ndjson"));
            let samples = dir.join("serverless").join(format!("gpu-{side}.ndjson"));
            if events.is_file() && samples.is_file() {
                serverless.insert(
                    side.to_string(),
                    Serving {
                        events: read_ndjson(&events)?,
                        samples: read_ndjson(&samples)?,
                    },
                );
            }
        }
        if gap.is_empty() && serverless.is_empty() {
            bail!("{}: no gap or serverless results", dir.display());
        }
        Ok(Self {
            meta,
            gap,
            serverless,
        })
    }

    pub fn rows(&self) -> Vec<GpuRow> {
        let mut rows = self.gap_rows();
        rows.extend(self.serverless_rows());
        rows
    }

    fn base_extra(&self) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        m.insert("box".into(), json!(self.meta.box_type));
        m.insert("gpu".into(), json!(self.meta.gpu));
        m.insert("smoke".into(), json!(self.meta.smoke));
        m.insert("wgpu".into(), json!(self.meta.wgpu.trim()));
        m.insert("llama_cpp".into(), json!(self.meta.llama_cpp));
        m
    }

    /// The headline model keeps `{box}/{step}`; every other model's rows add
    /// it: `{box}/{model}/{step}`.
    pub fn gap_param(&self, model: &str, step: Option<&str>) -> String {
        let b = &self.meta.box_type;
        let headline = self.meta.gap_model.as_deref() == Some(model);
        match (headline, step) {
            (true, Some(step)) => format!("{b}/{step}"),
            (true, None) => b.clone(),
            (false, Some(step)) => format!("{b}/{model}/{step}"),
            (false, None) => format!("{b}/{model}"),
        }
    }

    fn gap_rows(&self) -> Vec<GpuRow> {
        let mut rows = Vec::new();
        for (model, gap) in &self.gap {
            let extra = || {
                let mut e = self.base_extra();
                e.insert("model".into(), json!(model));
                e
            };
            // test -> step -> (mean, stddev)
            let mut by_test: BTreeMap<String, BTreeMap<&str, (f64, f64)>> = BTreeMap::new();
            for (step, tests) in &gap.steps {
                for t in tests {
                    let mut e = extra();
                    e.insert("backend".into(), json!(t.backends));
                    e.insert("stddev".into(), json!(t.stddev_ts));
                    e.insert("reps".into(), json!(t.reps()));
                    e.insert("threads".into(), json!(t.n_threads));
                    if step == "wasm"
                        && let Some(r) = &gap.wasm_report
                    {
                        e.insert("exit_ok".into(), json!(r.exit_code == Some(0)));
                    }
                    rows.push(GpuRow {
                        group: format!("llama-{}", t.name()),
                        param: self.gap_param(model, Some(step)),
                        metric: "tokens_per_s",
                        value: t.avg_ts,
                        unit: "tok/s",
                        better: Better::Higher,
                        extra: e,
                    });
                    by_test
                        .entry(t.name())
                        .or_default()
                        .insert(step.as_str(), (t.avg_ts, t.stddev_ts));
                }
            }
            for (test, steps) in &by_test {
                let gaps = [
                    ("webgpu_gap_pct", "vendor", "webgpu-native"),
                    ("wasm_overhead_pct", "webgpu-native", "wasm"),
                    ("native_gap_pct", "vendor", "wasm"),
                ];
                for (metric, from, to) in gaps {
                    if let (Some((a, sa)), Some((z, sz))) = (steps.get(from), steps.get(to))
                        && *a > 0.0
                    {
                        // A gap counts only when it is larger than the two
                        // steps' combined run-to-run spread.
                        let noise = sa.hypot(*sz);
                        let mut e = extra();
                        e.insert("noise_pct".into(), json!(noise / a * 100.0));
                        e.insert("significant".into(), json!((a - z).abs() > noise));
                        rows.push(GpuRow {
                            group: format!("llama-{test}"),
                            param: self.gap_param(model, None),
                            metric,
                            value: (a - z) / a * 100.0,
                            unit: "%",
                            better: Better::Lower,
                            extra: e,
                        });
                    }
                }
            }
            if let (Some(r), Some(t)) = (
                &gap.wasm_report,
                gap.steps.get("wasm").and_then(|t| t.first()),
            ) {
                for (metric, value) in [
                    ("model_size", t.model_size),
                    ("guest_memory_peak", r.guest_memory_peak),
                ] {
                    rows.push(GpuRow {
                        group: "llama-memory".into(),
                        param: self.gap_param(model, Some("wasm")),
                        metric,
                        value: value as f64,
                        unit: "B",
                        better: Better::Lower,
                        extra: extra(),
                    });
                }
            }
        }
        rows
    }

    fn serverless_rows(&self) -> Vec<GpuRow> {
        let mut rows = Vec::new();
        let b = &self.meta.box_type;
        for (side, s) in &self.serverless {
            let ok = s.events.iter().all(|e| match e {
                Event::TurnEnd { exit_code, .. } => *exit_code == Some(0),
                _ => true,
            });
            let mut push = |param: String,
                            metric,
                            value,
                            unit,
                            better,
                            model: Option<&str>,
                            more: Option<(&str, Value)>| {
                let mut e = self.base_extra();
                e.insert("turns_ok".into(), json!(ok));
                if let Some(model) = model {
                    e.insert("model".into(), json!(model));
                }
                if let Some((k, v)) = more {
                    e.insert(k.into(), v);
                }
                rows.push(GpuRow {
                    group: "serverless".into(),
                    param,
                    metric,
                    value,
                    unit,
                    better,
                    extra: e,
                });
            };
            for (model, t) in turn_times(s) {
                let param = format!("{b}/{side}/{model}");
                let metrics = [
                    ("turn_ms", t.turn),
                    ("start_ms", t.resolve),
                    ("load_ms", t.load),
                    ("first_token_ms", t.first_token),
                    ("ttft_ms", t.ttft),
                ];
                for (metric, values) in metrics {
                    if let Some(v) = median(&values) {
                        push(
                            param.clone(),
                            metric,
                            v,
                            "ms",
                            Better::Lower,
                            Some(&model),
                            Some(("turns", json!(values.len()))),
                        );
                    }
                }
            }
            let param = format!("{b}/{side}");
            let mem = memory(s);
            if let Some(v) = mem.host_idle {
                push(
                    param.clone(),
                    "host_idle_gpu_mem",
                    v,
                    "B",
                    Better::Lower,
                    None,
                    None,
                );
            }
            if let Some(v) = mem.idle {
                push(
                    param.clone(),
                    "idle_gpu_mem",
                    v,
                    "B",
                    Better::Lower,
                    None,
                    None,
                );
            }
            if let Some(v) = mem.peak {
                let timeline = timeline(s, mem.baseline);
                push(
                    param,
                    "peak_gpu_mem",
                    v,
                    "B",
                    Better::Lower,
                    None,
                    Some(("timeline", timeline)),
                );
            }
        }
        rows
    }
}

fn load_gap(dir: &Path) -> Result<Gap> {
    let mut gap = Gap::default();
    let mut reports: Vec<WasmReport> = Vec::new();
    for step in STEPS.into_iter().chain([THREADS_STEP]) {
        let mut runs = Vec::new();
        for path in reps(dir, step, ".json")? {
            // A run that crashed leaves an empty or partial file: skip it.
            if let Ok(tests) = read_json::<Vec<LlamaTest>>(&path)
                && !tests.is_empty()
            {
                runs.push(tests);
            }
        }
        if !runs.is_empty() {
            gap.steps.insert(step.to_string(), merge_reps(runs));
        }
    }
    for path in reps(dir, "wasm", ".report.json")? {
        reports.push(read_json(&path)?);
    }
    gap.wasm_report = reports
        .iter()
        .map(|r| r.guest_memory_peak)
        .max()
        .map(|guest_memory_peak| WasmReport {
            exit_code: if reports.iter().all(|r| r.exit_code == Some(0)) {
                Some(0)
            } else {
                reports.iter().find_map(|r| r.exit_code.filter(|c| *c != 0))
            },
            guest_memory_peak,
        });
    Ok(gap)
}

/// `{step}.r{N}{suffix}` files in `dir`, in repetition order.
fn reps(dir: &Path, step: &str, suffix: &str) -> Result<Vec<std::path::PathBuf>> {
    let prefix = format!("{step}.r");
    let mut found: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(n) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
            .and_then(|n| n.parse().ok())
        {
            found.push((n, path));
        }
    }
    found.sort();
    Ok(found.into_iter().map(|(_, p)| p).collect())
}

/// One model's turn times, in ms, over its turns that exited 0.
#[derive(Debug, Default)]
pub struct TurnTimes {
    /// Start to exit: workload start plus `wasi:cli/run`, or the process.
    pub turn: Vec<f64>,
    /// Workload start (compile, link, bind). wasmCloud side only.
    pub resolve: Vec<f64>,
    pub load: Vec<f64>,
    pub first_token: Vec<f64>,
    /// Start, model load and first token.
    pub ttft: Vec<f64>,
}

pub fn turn_times(s: &Serving) -> BTreeMap<String, TurnTimes> {
    let mut out: BTreeMap<String, TurnTimes> = BTreeMap::new();
    for e in &s.events {
        if let Event::TurnEnd {
            name,
            exit_code: Some(0),
            resolve_ms,
            run_ms,
            load_ms,
            first_token_ms,
            ..
        } = e
        {
            let t = out.entry(name.clone()).or_default();
            let start = resolve_ms.unwrap_or(0.0);
            t.turn.push(start + run_ms);
            t.resolve.extend(*resolve_ms);
            t.load.extend(*load_ms);
            t.first_token.extend(*first_token_ms);
            if let (Some(l), Some(f)) = (load_ms, first_token_ms) {
                t.ttft.push(start + l + f);
            }
        }
    }
    out
}

#[derive(Debug, Default)]
pub struct Memory {
    /// Median sample before the first event: what the box holds unprompted.
    pub baseline: u64,
    /// The most held over the baseline from the first turn on.
    pub peak: Option<f64>,
    /// Median held over the baseline in the back half of each idle window
    /// after a turn, once whatever was being released has gone.
    pub idle: Option<f64>,
    /// The same, for the idle window before the first turn: what the
    /// started host holds before it has served anything.
    pub host_idle: Option<f64>,
}

pub fn memory(s: &Serving) -> Memory {
    let Some(start) = s.events.iter().map(event_t).min() else {
        return Memory::default();
    };
    let before: Vec<f64> = s
        .samples
        .iter()
        .filter(|x| x.t_ms < start)
        .map(|x| x.used as f64)
        .collect();
    let baseline = median(&before).unwrap_or(0.0) as u64;
    let over = |x: &GpuSample| x.used.saturating_sub(baseline) as f64;
    let first_turn = s
        .events
        .iter()
        .find_map(|e| match e {
            Event::TurnStart { t_ms, .. } => Some(*t_ms),
            _ => None,
        })
        .unwrap_or(u64::MAX);
    let end = s.events.iter().map(event_t).max().unwrap_or(start);
    let peak = s
        .samples
        .iter()
        .filter(|x| x.t_ms >= first_turn && x.t_ms <= end)
        .map(over)
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
    let idle_back_half = |after_first_turn: bool| -> Option<f64> {
        let samples: Vec<f64> = s
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Idle { t_ms, until_ms } if (*t_ms >= first_turn) == after_first_turn => {
                    Some((t_ms + (until_ms - t_ms) / 2, *until_ms))
                }
                _ => None,
            })
            .flat_map(|(from, until)| {
                s.samples
                    .iter()
                    .filter(move |x| x.t_ms >= from && x.t_ms <= until)
                    .map(over)
            })
            .collect();
        median(&samples)
    };
    Memory {
        baseline,
        peak,
        idle: idle_back_half(true),
        host_idle: idle_back_half(false),
    }
}

fn event_t(e: &Event) -> u64 {
    match e {
        Event::Ready { t_ms }
        | Event::TurnStart { t_ms, .. }
        | Event::TurnEnd { t_ms, .. }
        | Event::Idle { t_ms, .. }
        | Event::Stopped { t_ms } => *t_ms,
    }
}

/// GPU memory over the baseline across the whole run, at most
/// [`TIMELINE_POINTS`] points (each the max of the samples it covers, so
/// short peaks survive), with the turns and idle windows to mark on it.
/// Times are milliseconds from the first sample.
pub fn timeline(s: &Serving, baseline: u64) -> Value {
    let t0 = s.samples.first().map_or(0, |x| x.t_ms);
    let per = s.samples.len().div_ceil(TIMELINE_POINTS).max(1);
    let points: Vec<Value> = s
        .samples
        .chunks(per)
        .filter_map(|c| {
            let t = c.first()?.t_ms - t0;
            let used = c.iter().map(|x| x.used).max()?.saturating_sub(baseline);
            Some(json!([t, used]))
        })
        .collect();
    let mut turns = Vec::new();
    let mut open: BTreeMap<&str, u64> = BTreeMap::new();
    let mut idle = Vec::new();
    for e in &s.events {
        match e {
            Event::TurnStart { t_ms, name } => {
                open.insert(name, *t_ms);
            }
            Event::TurnEnd {
                t_ms,
                name,
                exit_code,
                ..
            } => {
                if let Some(start) = open.remove(name.as_str()) {
                    turns.push(json!({
                        "name": name,
                        "start": start.saturating_sub(t0),
                        "end": t_ms.saturating_sub(t0),
                        "ok": *exit_code == Some(0),
                    }));
                }
            }
            Event::Idle { t_ms, until_ms } => {
                idle.push(json!([
                    t_ms.saturating_sub(t0),
                    until_ms.saturating_sub(t0)
                ]));
            }
            Event::Ready { .. } | Event::Stopped { .. } => {}
        }
    }
    json!({ "points": points, "turns": turns, "idle": idle })
}

fn median(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    if v.len().is_multiple_of(2) {
        Some((v.get(mid - 1)? + v.get(mid)?) / 2.0)
    } else {
        v.get(mid).copied()
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn read_ndjson<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l).with_context(|| format!("{}:{}", path.display(), i + 1))
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;

    fn sample(t_ms: u64, used: u64) -> GpuSample {
        GpuSample { t_ms, used }
    }

    fn end(t_ms: u64, name: &str, exit_code: i32, resolve_ms: f64, run_ms: f64) -> Event {
        Event::TurnEnd {
            t_ms,
            name: name.into(),
            exit_code: Some(exit_code),
            resolve_ms: Some(resolve_ms),
            run_ms,
            load_ms: Some(50.0),
            first_token_ms: Some(20.0),
        }
    }

    fn serving() -> Serving {
        Serving {
            events: vec![
                Event::Ready { t_ms: 60 },
                Event::Idle {
                    t_ms: 60,
                    until_ms: 100,
                },
                Event::TurnStart {
                    t_ms: 100,
                    name: "a".into(),
                },
                end(200, "a", 0, 10.0, 90.0),
                Event::Idle {
                    t_ms: 200,
                    until_ms: 400,
                },
            ],
            samples: vec![
                sample(0, 1000),
                sample(50, 1000),
                sample(90, 1300),
                sample(150, 9000),
                sample(250, 5000),
                sample(350, 1100),
                sample(390, 1100),
            ],
        }
    }

    #[test]
    fn memory_is_measured_over_the_baseline() {
        let m = memory(&serving());
        assert_eq!(m.baseline, 1000);
        assert_eq!(m.peak, Some(8000.0));
        // Only the back half of each idle window counts.
        assert_eq!(m.idle, Some(100.0));
        assert_eq!(m.host_idle, Some(300.0));
    }

    #[test]
    fn timeline_keeps_peaks_and_marks_turns() {
        let t = timeline(&serving(), 1000);
        assert_eq!(t["points"][3], json!([150, 8000]));
        assert_eq!(t["turns"][0]["start"], 100);
        assert_eq!(t["turns"][0]["ok"], true);
        assert_eq!(t["idle"][1], json!([200, 400]));
    }

    #[test]
    fn timeline_downsamples_to_its_cap() {
        let s = Serving {
            events: Vec::new(),
            samples: (0..1801).map(|i| sample(i, i)).collect(),
        };
        let t = timeline(&s, 0);
        let n = t["points"].as_array().map_or(0, Vec::len);
        assert!(n <= TIMELINE_POINTS, "{n} points");
        // Each point is the max of what it covers.
        assert_eq!(t["points"][0], json!([0, 3]));
    }

    pub(crate) fn run(
        gap: BTreeMap<String, BTreeMap<String, Vec<LlamaTest>>>,
        wasmcloud: Option<Serving>,
    ) -> GpuRun {
        GpuRun {
            meta: Metadata {
                box_type: "metal".into(),
                gpu: "test".into(),
                gap_model: Some("big".into()),
                smoke: false,
                wgpu: String::new(),
                llama_cpp: String::new(),
                failed: Vec::new(),
            },
            gap: gap
                .into_iter()
                .map(|(m, steps)| {
                    (
                        m,
                        Gap {
                            steps,
                            wasm_report: None,
                        },
                    )
                })
                .collect(),
            serverless: wasmcloud
                .map(|s| ("wasmcloud".to_string(), s))
                .into_iter()
                .collect(),
        }
    }

    pub(crate) fn llama(n_prompt: u64, n_gen: u64, avg_ts: f64) -> LlamaTest {
        LlamaTest {
            n_prompt,
            n_gen,
            avg_ts,
            stddev_ts: 0.0,
            samples_ts: Vec::new(),
            backends: String::new(),
            model_size: 0,
            n_threads: 1,
        }
    }

    /// One model's three steps, each with one tg128 result.
    pub(crate) fn steps(vendor: f64, native: f64, wasm: f64) -> BTreeMap<String, Vec<LlamaTest>> {
        BTreeMap::from([
            ("vendor".to_string(), vec![llama(0, 128, vendor)]),
            ("webgpu-native".to_string(), vec![llama(0, 128, native)]),
            ("wasm".to_string(), vec![llama(0, 128, wasm)]),
        ])
    }

    fn find<'a>(rows: &'a [GpuRow], param: &str, metric: &str) -> Option<&'a GpuRow> {
        rows.iter().find(|r| r.param == param && r.metric == metric)
    }

    #[test]
    fn turn_times_skip_failed_turns_and_split_per_model() {
        let mut s = serving();
        s.events.push(end(500, "a", 1, 1.0, 1.0));
        s.events.push(end(600, "b", 0, 5.0, 45.0));
        let rows = run(BTreeMap::new(), Some(s)).rows();
        let value = |param: &str, metric: &str| find(&rows, param, metric).map(|r| r.value);
        assert_eq!(value("metal/wasmcloud/a", "turn_ms"), Some(100.0));
        assert_eq!(value("metal/wasmcloud/a", "start_ms"), Some(10.0));
        assert_eq!(value("metal/wasmcloud/a", "ttft_ms"), Some(80.0));
        assert_eq!(value("metal/wasmcloud/b", "turn_ms"), Some(50.0));
        assert!(rows.iter().all(|r| r.metric != "models_per_gpu"));
    }

    #[test]
    fn native_turns_have_no_start_time() {
        let events = vec![Event::TurnEnd {
            t_ms: 1,
            name: "a".into(),
            exit_code: Some(0),
            resolve_ms: None,
            run_ms: 30.0,
            load_ms: Some(10.0),
            first_token_ms: Some(5.0),
        }];
        let t = turn_times(&Serving {
            events,
            samples: Vec::new(),
        });
        let a = t.get("a");
        assert_eq!(a.map(|a| a.resolve.len()), Some(0));
        assert_eq!(a.and_then(|a| a.ttft.first().copied()), Some(15.0));
    }

    #[test]
    fn headline_model_keeps_short_params() {
        let gap = BTreeMap::from([
            ("big".to_string(), steps(100.0, 50.0, 49.0)),
            ("small".to_string(), steps(200.0, 80.0, 60.0)),
        ]);
        let rows = run(gap, None).rows();
        assert!(find(&rows, "metal/wasm", "tokens_per_s").is_some());
        assert!(find(&rows, "metal", "wasm_overhead_pct").is_some());
        let small = find(&rows, "metal/small", "wasm_overhead_pct").map(|r| r.value);
        assert_eq!(small, Some(25.0));
        assert!(find(&rows, "metal/small/wasm", "tokens_per_s").is_some());
    }

    #[test]
    fn gaps_inside_the_noise_are_not_significant() {
        let mut s = steps(100.0, 50.0, 49.0);
        if let Some(t) = s.get_mut("wasm").and_then(|t| t.first_mut()) {
            t.stddev_ts = 2.0;
        }
        let rows = run(BTreeMap::from([("big".to_string(), s)]), None).rows();
        let overhead = find(&rows, "metal", "wasm_overhead_pct");
        assert_eq!(
            overhead.and_then(|r| r.extra.get("significant")),
            Some(&json!(false))
        );
        let vendor = find(&rows, "metal", "webgpu_gap_pct");
        assert_eq!(
            vendor.and_then(|r| r.extra.get("significant")),
            Some(&json!(true))
        );
    }

    #[test]
    fn merge_reps_pools_samples() {
        let rep = |ts: f64| {
            let mut t = llama(0, 128, ts);
            t.samples_ts = vec![ts];
            vec![t]
        };
        let merged = merge_reps(vec![rep(10.0), rep(12.0), rep(14.0)]);
        assert_eq!(merged.len(), 1);
        let t = merged.first();
        assert_eq!(t.map(|t| t.avg_ts), Some(12.0));
        assert_eq!(t.map(|t| t.stddev_ts), Some(2.0));
        assert_eq!(t.map(LlamaTest::reps), Some(3));
    }

    #[test]
    fn load_reads_interleaved_reps() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let model = dir.path().join("gap/m");
        std::fs::create_dir_all(&model)?;
        std::fs::write(
            dir.path().join("metadata.json"),
            r#"{"box":"metal","gpu":"g","gap_model":"m"}"#,
        )?;
        let test = |ts: f64| {
            format!(
                r#"[{{"n_prompt":0,"n_gen":16,"avg_ts":{ts},"stddev_ts":0,"samples_ts":[{ts}],"backends":"WebGPU","model_size":1,"n_threads":1}}]"#
            )
        };
        std::fs::write(model.join("wasm.r1.json"), test(10.0))?;
        std::fs::write(model.join("wasm.r2.json"), test(20.0))?;
        // A crashed repetition leaves an empty file.
        std::fs::write(model.join("wasm.r3.json"), "")?;
        std::fs::write(
            model.join("wasm.r1.report.json"),
            r#"{"exit_code":0,"guest_memory_peak":5}"#,
        )?;
        std::fs::write(
            model.join("wasm.r2.report.json"),
            r#"{"exit_code":0,"guest_memory_peak":7}"#,
        )?;
        let run = GpuRun::load(dir.path())?;
        let gap = run.gap.get("m").context("model m")?;
        let wasm = gap.steps.get("wasm").and_then(|t| t.first());
        assert_eq!(wasm.map(|t| t.avg_ts), Some(15.0));
        assert_eq!(
            gap.wasm_report.as_ref().map(|r| r.guest_memory_peak),
            Some(7)
        );
        Ok(())
    }

    #[test]
    fn median_of_even_and_odd() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }
}
