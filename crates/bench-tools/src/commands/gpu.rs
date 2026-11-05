//! `bench-tools gpu …` — render and export `scripts/gpubench` runs.
//!
//! - `report <dir>`: the gap table and the serverless numbers (`--markdown`
//!   for `$GITHUB_STEP_SUMMARY`)
//! - `jsonl <dir>`: history.json rows

use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;

use crate::gpu::{GpuRow, GpuRun, STEPS};
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

    let mut tests: Vec<&str> = rows
        .iter()
        .filter(|r| r.metric == "tokens_per_s")
        .map(|r| r.group.as_str())
        .collect();
    tests.sort_unstable();
    tests.dedup();
    if !tests.is_empty() {
        let model = run.meta.gap_model.as_deref().unwrap_or("?");
        let _ = writeln!(s, "How far from native, {model} (tok/s):\n");
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
                "{:<14}{:>12}{:>16}{:>12}{:>24}",
                header[0], header[1], header[2], header[3], header[4]
            );
        }
        for test in tests {
            let cell = |step: &str| {
                find(test, &format!("{b}/{step}"), "tokens_per_s")
                    .map_or("—".to_string(), |r| format!("{:.1}", r.value))
            };
            let overhead = find(test, b, "wasm_overhead_pct")
                .map_or("—".to_string(), |r| format!("{:+.1}%", -r.value));
            let name = test.trim_start_matches("llama-");
            let [v, n, w] = STEPS.map(cell);
            if markdown {
                let _ = writeln!(s, "| {name} | {v} | {n} | {w} | {overhead} |");
            } else {
                let _ = writeln!(s, "{name:<14}{v:>12}{n:>16}{w:>12}{overhead:>24}");
            }
        }
        if let (Some(size), Some(peak)) = (
            find("llama-memory", &format!("{b}/wasm"), "model_size"),
            find("llama-memory", &format!("{b}/wasm"), "guest_memory_peak"),
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

    let param = format!("{b}/wasmcloud");
    let cold = find("serverless", &param, "cold_start_ttft_ms");
    let idle = find("serverless", &param, "idle_gpu_mem");
    let peak = find("serverless", &param, "peak_gpu_mem");
    let per = find("serverless", &param, "models_per_gpu");
    if cold.is_some() || idle.is_some() {
        let _ = writeln!(
            s,
            "{}wasmcloud: cold start to first token {}, GPU memory {} busy / {} idle, {} model(s) on the GPU",
            if markdown { "- " } else { "" },
            cold.map_or("—".to_string(), |r| format!("{:.0} ms", r.value)),
            peak.map_or("—".to_string(), |r| bytes(r.value)),
            idle.map_or("—".to_string(), |r| bytes(r.value)),
            per.map_or("—".to_string(), |r| format!("{}", r.value)),
        );
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
    use crate::gpu::tests::{llama, run};

    #[test]
    fn report_lists_each_test_once() {
        let gap = BTreeMap::from(STEPS.map(|step| {
            (
                step.to_string(),
                vec![llama(512, 0, 100.0), llama(0, 128, 10.0)],
            )
        }));
        let text = report(&run(gap, None), true);
        assert_eq!(text.matches("| pp512 |").count(), 1, "{text}");
        assert_eq!(text.matches("| tg128 |").count(), 1, "{text}");
    }
}
