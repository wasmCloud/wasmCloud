//! `bench-tools gpu …` — render and export `scripts/gpubench` runs.
//!
//! - `report <dir>`: a gap table per model and the serverless numbers per
//!   side (`--markdown` for `$GITHUB_STEP_SUMMARY`)
//! - `jsonl <dir>`: history.json rows

use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;

use crate::gpu::{GpuRow, GpuRun, SIDES, STEPS, THREADS_STEP};
use crate::meta::Meta;

#[derive(Debug, clap::Subcommand)]
pub enum Cmd {
    /// Summarize one result directory.
    Report {
        dir: PathBuf,
        /// Emit GitHub-flavored markdown instead of terminal text.
        #[arg(long)]
        markdown: bool,
    },
    /// Emit one history.json row per measurement.
    Jsonl { dir: PathBuf },
}

pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Report { dir, markdown } => print!("{}", report(&GpuRun::load(&dir)?, markdown)),
        Cmd::Jsonl { dir } => jsonl(&GpuRun::load(&dir)?)?,
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct Row<'a> {
    bench: &'static str,
    group: &'a str,
    param: &'a str,
    #[serde(flatten)]
    meta: &'a Meta,
    metric: &'a str,
    value: f64,
    unit: &'a str,
    better: &'static str,
    #[serde(flatten)]
    extra: &'a serde_json::Map<String, serde_json::Value>,
}

fn jsonl(run: &GpuRun) -> Result<()> {
    let meta = Meta::capture()?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for r in run.rows() {
        let row = Row {
            bench: "gpubench",
            group: &r.group,
            param: &r.param,
            meta: &meta,
            metric: r.metric,
            value: r.value,
            unit: r.unit,
            better: r.better.as_str(),
            extra: &r.extra,
        };
        serde_json::to_writer(&mut out, &row)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

fn report(run: &GpuRun, markdown: bool) -> String {
    let rows = run.rows();
    let find = |group: &str, param: &str, metric: &str| -> Option<&GpuRow> {
        rows.iter()
            .find(|r| r.group == group && r.param == param && r.metric == metric)
    };
    let b = &run.meta.box_type;
    let mut s = String::new();
    let smoke = if run.meta.smoke { " (smoke)" } else { "" };
    let title = format!("wasmCloud GPU bench — {b} · {}{smoke}", run.meta.gpu);
    if markdown {
        let _ = writeln!(s, "### {title}\n");
    } else {
        let _ = writeln!(s, "\n{title}\n{}", "─".repeat(title.chars().count()));
    }
    if !run.meta.failed.is_empty() {
        let _ = writeln!(s, "Failed steps: {}\n", run.meta.failed.join(", "));
    }

    let headline = run.meta.gap_model.as_deref();
    let mut models: Vec<&str> = run.gap.keys().map(String::as_str).collect();
    models.sort_by_key(|m| Some(*m) != headline);
    for model in models {
        let mut tests: Vec<&str> = rows
            .iter()
            .filter(|r| {
                r.metric == "tokens_per_s"
                    && r.extra.get("model").and_then(|m| m.as_str()) == Some(model)
            })
            .map(|r| r.group.as_str())
            .collect();
        tests.sort_unstable();
        tests.dedup();
        if tests.is_empty() {
            continue;
        }
        let _ = writeln!(s, "How far from native, {model} (tok/s, mean ± stddev):\n");
        let header = [
            "test",
            "vendor",
            "webgpu-native",
            "wasm",
            "wasm vs native webgpu",
        ];
        if markdown {
            let _ = writeln!(s, "| {} |", header.join(" | "));
            let _ = writeln!(s, "|---|---:|---:|---:|---:|");
        } else {
            let _ = writeln!(
                s,
                "{:<14}{:>16}{:>16}{:>16}{:>24}",
                header[0], header[1], header[2], header[3], header[4]
            );
        }
        for test in tests {
            let cell = |step: &str| {
                find(test, &run.gap_param(model, Some(step)), "tokens_per_s").map_or(
                    "—".to_string(),
                    |r| {
                        let sd = r
                            .extra
                            .get("stddev")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        format!("{:.1} ± {sd:.1}", r.value)
                    },
                )
            };
            let overhead = find(test, &run.gap_param(model, None), "wasm_overhead_pct").map_or(
                "—".to_string(),
                |r| {
                    let noise = if r.extra.get("significant") == Some(&serde_json::json!(false)) {
                        " (noise)"
                    } else {
                        ""
                    };
                    format!("{:+.1}%{noise}", -r.value)
                },
            );
            let name = test.trim_start_matches("llama-");
            let [v, n, w] = STEPS.map(cell);
            if markdown {
                let _ = writeln!(s, "| {name} | {v} | {n} | {w} | {overhead} |");
            } else {
                let _ = writeln!(s, "{name:<14}{v:>16}{n:>16}{w:>16}{overhead:>24}");
            }
            if let Some(mt) = find(
                test,
                &run.gap_param(model, Some(THREADS_STEP)),
                "tokens_per_s",
            ) {
                let threads = mt
                    .extra
                    .get("threads")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let _ = writeln!(
                    s,
                    "{}webgpu-native on {threads} threads: {:.1}",
                    if markdown { "| | | " } else { "  " },
                    mt.value
                );
            }
        }
        let wasm = run.gap_param(model, Some("wasm"));
        if let (Some(size), Some(peak)) = (
            find("llama-memory", &wasm, "model_size"),
            find("llama-memory", &wasm, "guest_memory_peak"),
        ) {
            let _ = writeln!(
                s,
                "\nwasm guest: {} model, {} guest memory at peak",
                bytes(size.value),
                bytes(peak.value)
            );
        }
        s.push('\n');
    }

    for side in SIDES {
        let param = format!("{b}/{side}");
        let idle = find("serverless", &param, "idle_gpu_mem");
        let host = find("serverless", &param, "host_idle_gpu_mem");
        let peak = find("serverless", &param, "peak_gpu_mem");
        if peak.is_none() && idle.is_none() {
            continue;
        }
        let mb = |r: Option<&GpuRow>| r.map_or("—".to_string(), |r| bytes(r.value));
        let _ = writeln!(
            s,
            "{side}: GPU memory {} busy, {} idle between turns, {} before the first turn",
            mb(peak),
            mb(idle),
            mb(host),
        );
        let mut models: Vec<&str> = rows
            .iter()
            .filter(|r| r.group == "serverless" && r.param.starts_with(&format!("{param}/")))
            .filter_map(|r| r.extra.get("model").and_then(|m| m.as_str()))
            .collect();
        models.sort_unstable();
        models.dedup();
        for model in models {
            let p = format!("{param}/{model}");
            let ms = |metric: &str| {
                find("serverless", &p, metric)
                    .map_or("—".to_string(), |r| format!("{:.0} ms", r.value))
            };
            let _ = writeln!(
                s,
                "{}{model}: first token {} (start {}, load {}, prompt + token {}), whole turn {}",
                if markdown { "- " } else { "  " },
                ms("ttft_ms"),
                ms("start_ms"),
                ms("load_ms"),
                ms("first_token_ms"),
                ms("turn_ms"),
            );
        }
        s.push('\n');
    }
    s
}

fn bytes(v: f64) -> String {
    const GIB: f64 = (1u64 << 30) as f64;
    const MIB: f64 = (1u64 << 20) as f64;
    if v >= GIB {
        format!("{:.2} GiB", v / GIB)
    } else {
        format!("{:.0} MiB", v / MIB)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::gpu::tests::{run, steps};

    #[test]
    fn report_lists_each_test_once() {
        let gap = BTreeMap::from([("big".to_string(), steps(100.0, 50.0, 49.0))]);
        let text = report(&run(gap, None), true);
        assert_eq!(text.matches("| tg128 |").count(), 1, "{text}");
    }

    #[test]
    fn report_puts_the_headline_model_first() {
        let gap = BTreeMap::from([
            ("a-small".to_string(), steps(100.0, 50.0, 49.0)),
            ("big".to_string(), steps(10.0, 5.0, 4.9)),
        ]);
        let text = report(&run(gap, None), false);
        let big = text.find("native, big");
        let small = text.find("native, a-small");
        assert!(big.is_some() && small.is_some() && big < small, "{text}");
    }
}
