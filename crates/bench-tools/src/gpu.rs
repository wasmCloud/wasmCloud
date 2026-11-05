//! Reads a `scripts/gpubench/run.sh` result directory into history rows.
//!
//! - The native gap bench: tok/s for each step (`vendor`, `webgpu-native`,
//!   `wasm`), and the gaps between them.
//! - The serverless bench: cold start to first token, and GPU memory while
//!   busy and while idle, for wasmCloud's cold-started turns.
//!
//! GPU memory is reported over the idle baseline sampled before a run, so a
//! box's other GPU users (the window server on macOS) drop out.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::k6::Better;

/// Steps of the gap bench, in the order they're compared.
pub const STEPS: [&str; 3] = ["vendor", "webgpu-native", "wasm"];

/// Timeline points kept on the row that carries the memory-over-time chart.
const TIMELINE_POINTS: usize = 600;

#[derive(Debug, Deserialize)]
pub struct Metadata {
    pub box_type: String,
    pub gpu: String,
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
#[derive(Debug, Deserialize)]
pub struct LlamaTest {
    pub n_prompt: u64,
    pub n_gen: u64,
    pub avg_ts: f64,
    pub stddev_ts: f64,
    pub backends: String,
    pub model_size: u64,
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
    },
    Idle {
        t_ms: u64,
        until_ms: u64,
    },
    Stopped {
        t_ms: u64,
    },
}

/// The serverless bench's events and the GPU samples taken while it ran.
#[derive(Debug)]
pub struct Serving {
    pub events: Vec<Event>,
    pub samples: Vec<GpuSample>,
}

#[derive(Debug)]
pub struct GpuRun {
    pub meta: Metadata,
    /// llama-bench results per step; a step that didn't run is absent.
    pub gap: BTreeMap<String, Vec<LlamaTest>>,
    pub wasm_report: Option<WasmReport>,
    pub wasmcloud: Option<Serving>,
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
        for step in STEPS {
            let path = dir.join("gap/llama").join(format!("{step}.json"));
            if path.is_file() {
                // A step that crashed leaves an empty or partial file: absent.
                if let Ok(tests) = read_json::<Vec<LlamaTest>>(&path)
                    && !tests.is_empty()
                {
                    gap.insert(step.to_string(), tests);
                }
            }
        }
        let report = dir.join("gap/llama/wasm.report.json");
        let wasm_report = report.is_file().then(|| read_json(&report)).transpose()?;
        let serverless = dir.join("serverless");
        let wasmcloud = if serverless.join("wasmcloud.ndjson").is_file() {
            Some(Serving {
                events: read_ndjson(&serverless.join("wasmcloud.ndjson"))?,
                samples: read_ndjson(&serverless.join("gpu.ndjson"))?,
            })
        } else {
            None
        };
        if gap.is_empty() && wasmcloud.is_none() {
            bail!("{}: no gap or serverless results", dir.display());
        }
        Ok(Self {
            meta,
            gap,
            wasm_report,
            wasmcloud,
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

    fn gap_rows(&self) -> Vec<GpuRow> {
        let mut rows = Vec::new();
        let b = &self.meta.box_type;
        let model = self.meta.gap_model.clone().unwrap_or_default();
        let mut by_test: BTreeMap<String, BTreeMap<&str, f64>> = BTreeMap::new();
        for (step, tests) in &self.gap {
            for t in tests {
                let mut extra = self.base_extra();
                extra.insert("model".into(), json!(model));
                extra.insert("backend".into(), json!(t.backends));
                extra.insert("stddev".into(), json!(t.stddev_ts));
                if step == "wasm"
                    && let Some(r) = &self.wasm_report
                {
                    extra.insert("exit_ok".into(), json!(r.exit_code == Some(0)));
                }
                rows.push(GpuRow {
                    group: format!("llama-{}", t.name()),
                    param: format!("{b}/{step}"),
                    metric: "tokens_per_s",
                    value: t.avg_ts,
                    unit: "tok/s",
                    better: Better::Higher,
                    extra,
                });
                by_test
                    .entry(t.name())
                    .or_default()
                    .insert(step.as_str(), t.avg_ts);
            }
        }
        for (test, steps) in &by_test {
            let gaps = [
                ("webgpu_gap_pct", "vendor", "webgpu-native"),
                ("wasm_overhead_pct", "webgpu-native", "wasm"),
                ("native_gap_pct", "vendor", "wasm"),
            ];
            for (metric, from, to) in gaps {
                if let (Some(a), Some(z)) = (steps.get(from), steps.get(to))
                    && *a > 0.0
                {
                    let mut extra = self.base_extra();
                    extra.insert("model".into(), json!(model));
                    rows.push(GpuRow {
                        group: format!("llama-{test}"),
                        param: b.clone(),
                        metric,
                        value: (a - z) / a * 100.0,
                        unit: "%",
                        better: Better::Lower,
                        extra,
                    });
                }
            }
        }
        if let (Some(r), Some(tests)) = (&self.wasm_report, self.gap.get("wasm"))
            && let Some(t) = tests.first()
        {
            for (metric, value) in [
                ("model_size", t.model_size),
                ("guest_memory_peak", r.guest_memory_peak),
            ] {
                let mut extra = self.base_extra();
                extra.insert("model".into(), json!(model));
                rows.push(GpuRow {
                    group: "llama-memory".into(),
                    param: format!("{b}/wasm"),
                    metric,
                    value: value as f64,
                    unit: "B",
                    better: Better::Lower,
                    extra,
                });
            }
        }
        rows
    }

    fn serverless_rows(&self) -> Vec<GpuRow> {
        let mut rows = Vec::new();
        let b = &self.meta.box_type;
        if let Some(s) = &self.wasmcloud {
            let first = s.events.iter().find_map(|e| match e {
                Event::TurnStart { name, .. } => Some(name.clone()),
                _ => None,
            });
            let ok = s.events.iter().all(|e| match e {
                Event::TurnEnd { exit_code, .. } => *exit_code == Some(0),
                _ => true,
            });
            let models: std::collections::BTreeSet<&str> = s
                .events
                .iter()
                .filter_map(|e| match e {
                    Event::TurnEnd {
                        name,
                        exit_code: Some(0),
                        ..
                    } => Some(name.as_str()),
                    _ => None,
                })
                .collect();
            let cold: Vec<f64> = s
                .events
                .iter()
                .filter_map(|e| match e {
                    Event::TurnEnd {
                        name,
                        exit_code: Some(0),
                        resolve_ms,
                        run_ms,
                        ..
                    } if Some(name) == first.as_ref() => Some(resolve_ms.unwrap_or(0.0) + run_ms),
                    _ => None,
                })
                .collect();
            let mut push = |metric, value, unit, better, extra: Option<serde_json::Map<_, _>>| {
                let mut e = extra.unwrap_or_else(|| self.base_extra());
                e.insert("turns_ok".into(), json!(ok));
                e.insert("model".into(), json!(first.clone().unwrap_or_default()));
                rows.push(GpuRow {
                    group: "serverless".into(),
                    param: format!("{b}/wasmcloud"),
                    metric,
                    value,
                    unit,
                    better,
                    extra: e,
                });
            };
            if let Some(v) = median(&cold) {
                push("cold_start_ttft_ms", v, "ms", Better::Lower, None);
            }
            push(
                "models_per_gpu",
                models.len() as f64,
                "",
                Better::Higher,
                None,
            );
            let mem = memory(s);
            if let Some(v) = mem.idle {
                push("idle_gpu_mem", v, "B", Better::Lower, None);
            }
            if let Some(v) = mem.peak {
                let mut extra = self.base_extra();
                extra.insert("timeline".into(), timeline(s, mem.baseline));
                push("peak_gpu_mem", v, "B", Better::Lower, Some(extra));
            }
        }
        rows
    }
}

#[derive(Debug, Default)]
pub struct Memory {
    /// Median sample before the first event: what the box holds unprompted.
    pub baseline: u64,
    /// The most held over the baseline while serving.
    pub peak: Option<f64>,
    /// Median held over the baseline in the back half of each idle window,
    /// once whatever was being released has gone.
    pub idle: Option<f64>,
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
    let end = s.events.iter().map(event_t).max().unwrap_or(start);
    let peak = s
        .samples
        .iter()
        .filter(|x| x.t_ms >= start && x.t_ms <= end)
        .map(over)
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
    let idle_samples: Vec<f64> = s
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Idle { t_ms, until_ms } => Some((t_ms + (until_ms - t_ms) / 2, *until_ms)),
            _ => None,
        })
        .flat_map(|(from, until)| {
            s.samples
                .iter()
                .filter(move |x| x.t_ms >= from && x.t_ms <= until)
                .map(over)
        })
        .collect();
    Memory {
        baseline,
        peak,
        idle: median(&idle_samples),
    }
}

fn event_t(e: &Event) -> u64 {
    match e {
        Event::TurnStart { t_ms, .. }
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
            Event::Stopped { .. } => {}
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

    fn serving() -> Serving {
        Serving {
            events: vec![
                Event::TurnStart {
                    t_ms: 100,
                    name: "a".into(),
                },
                Event::TurnEnd {
                    t_ms: 200,
                    name: "a".into(),
                    exit_code: Some(0),
                    resolve_ms: Some(10.0),
                    run_ms: 90.0,
                },
                Event::Idle {
                    t_ms: 200,
                    until_ms: 400,
                },
            ],
            samples: vec![
                sample(0, 1000),
                sample(50, 1000),
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
        // Only the back half of the idle window counts.
        assert_eq!(m.idle, Some(100.0));
    }

    #[test]
    fn timeline_keeps_peaks_and_marks_turns() {
        let t = timeline(&serving(), 1000);
        assert_eq!(t["points"][2], json!([150, 8000]));
        assert_eq!(t["turns"][0]["start"], 100);
        assert_eq!(t["turns"][0]["ok"], true);
        assert_eq!(t["idle"][0], json!([200, 400]));
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

    pub(crate) fn run(gap: BTreeMap<String, Vec<LlamaTest>>, wasmcloud: Option<Serving>) -> GpuRun {
        GpuRun {
            meta: Metadata {
                box_type: "metal".into(),
                gpu: "test".into(),
                gap_model: None,
                smoke: false,
                wgpu: String::new(),
                llama_cpp: String::new(),
                failed: Vec::new(),
            },
            gap,
            wasm_report: None,
            wasmcloud,
        }
    }

    pub(crate) fn llama(n_prompt: u64, n_gen: u64, avg_ts: f64) -> LlamaTest {
        LlamaTest {
            n_prompt,
            n_gen,
            avg_ts,
            stddev_ts: 0.0,
            backends: String::new(),
            model_size: 0,
        }
    }

    #[test]
    fn cold_start_skips_failed_turns() {
        let mut s = serving();
        s.events.push(Event::TurnEnd {
            t_ms: 500,
            name: "a".into(),
            exit_code: Some(1),
            resolve_ms: Some(1.0),
            run_ms: 1.0,
        });
        let rows = run(BTreeMap::new(), Some(s)).rows();
        let cold = rows
            .iter()
            .find(|r| r.metric == "cold_start_ttft_ms")
            .map(|r| r.value);
        assert_eq!(cold, Some(100.0));
    }

    #[test]
    fn median_of_even_and_odd() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
    }
}
