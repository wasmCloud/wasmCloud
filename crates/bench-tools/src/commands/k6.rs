//! `bench-tools k6 …` — render, export and compare `scripts/k6bench` runs.
//!
//! - `report <dir>`: terminal summary (`--markdown` for `$GITHUB_STEP_SUMMARY`)
//! - `jsonl <dir>`: history.json rows, one per headline metric
//! - `delta <a> <b>`: per-metric change between two runs

use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::k6::{Better, Metric, Run, ScenarioStats};
use crate::meta::Meta;

/// Changes smaller than this are reported as noise, not as a move.
const NOISE_PCT: f64 = 2.0;

#[derive(Debug, clap::Subcommand)]
pub enum Cmd {
    /// Summarize one run directory.
    Report {
        dir: PathBuf,
        /// Emit GitHub-flavored markdown instead of terminal text.
        #[arg(long)]
        markdown: bool,
    },
    /// Emit one history.json row per headline metric.
    Jsonl { dir: PathBuf },
    /// Compare two run directories (baseline first).
    Delta {
        baseline: PathBuf,
        candidate: PathBuf,
        #[arg(long)]
        markdown: bool,
    },
}

pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Report { dir, markdown } => {
            let run = Run::load(&dir)?;
            print!("{}", report(&run, Style::pick(markdown)));
        }
        Cmd::Jsonl { dir } => jsonl(&Run::load(&dir)?)?,
        Cmd::Delta {
            baseline,
            candidate,
            markdown,
        } => {
            let a = Run::load(&baseline)?;
            let b = Run::load(&candidate)?;
            if a.summary.scenario != b.summary.scenario || a.param() != b.param() {
                bail!(
                    "runs don't compare: {} {} vs {} {}",
                    a.summary.scenario,
                    a.param(),
                    b.summary.scenario,
                    b.param()
                );
            }
            print!("{}", delta(&a, &b, Style::pick(markdown)));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Markdown,
    Color,
    Plain,
}

impl Style {
    fn pick(markdown: bool) -> Self {
        if markdown {
            Style::Markdown
        } else if std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
            Style::Color
        } else {
            Style::Plain
        }
    }

    fn paint(self, code: &str, text: &str) -> String {
        match self {
            Style::Color => format!("\x1b[{code}m{text}\x1b[0m"),
            _ => text.to_string(),
        }
    }

    fn ok(self, ok: bool) -> String {
        if ok {
            self.paint("32", "✓")
        } else {
            self.paint("31", "✗")
        }
    }
}

fn report(run: &Run, style: Style) -> String {
    let mut s = String::new();
    let title = format!(
        "wasmCloud k6 bench — {} · {}",
        run.summary.scenario,
        run.param()
    );
    if style == Style::Markdown {
        let _ = writeln!(s, "### {title}\n");
    } else {
        let _ = writeln!(s, "\n{}", style.paint("1", &title));
        let _ = writeln!(s, "{}", "─".repeat(title.chars().count()));
    }

    let meta = &run.meta;
    let mut facts = vec![
        ("wash", meta.wash_image.clone()),
        ("k6", format!("{} ({})", meta.k6_version, meta.k6_mode)),
        ("target", meta.target.clone()),
        ("pinned", meta.pinned.to_string()),
    ];
    if let Some(ready) = meta.deploy_ready_s {
        facts.push(("workloads ready after", format!("{ready} s")));
    }
    facts.retain(|(_, v)| !v.is_empty() && v != " ()");
    for (k, v) in facts {
        let _ = match style {
            Style::Markdown => writeln!(s, "- **{k}:** `{v}`"),
            _ => writeln!(s, "{k:>22}  {v}"),
        };
    }
    s.push('\n');

    scenario_table(&mut s, run, style);

    let heading = |s: &mut String, text: &str| {
        let _ = match style {
            Style::Markdown => writeln!(s, "\n**{text}**\n"),
            _ => writeln!(s, "\n{}", style.paint("1", text)),
        };
    };
    heading(&mut s, "Headline");
    for m in run.metrics() {
        let _ = match style {
            Style::Markdown => writeln!(s, "- `{}` = {}", m.name, fmt_value(&m)),
            _ => writeln!(s, "  {:<26} {:>14}", m.name, fmt_value(&m)),
        };
    }

    if !run.samples.is_empty() {
        heading(&mut s, "Cluster (measured window)");
        for (label, suffix) in [
            ("hosts node", "-worker"),
            ("control-plane node", "-control-plane"),
            ("k6 container", "-k6"),
        ] {
            if let Some((cpu, mem)) = run.cluster_avg(|n| n.ends_with(suffix)) {
                let line = format!("{cpu:.2} cores avg, {mem:.0} MiB peak");
                let _ = match style {
                    Style::Markdown => writeln!(s, "- {label}: {line}"),
                    _ => writeln!(s, "  {label:<26} {line}"),
                };
            }
        }
    }

    heading(&mut s, "Verdict");
    let passed = run.thresholds_passed();
    let saturated = run.generator_saturated();
    let _ = writeln!(s, "  {} thresholds", style.ok(passed));
    let _ = writeln!(
        s,
        "  {} load generator kept up{}",
        style.ok(!saturated),
        if saturated {
            " — k6 was the bottleneck; don't publish these numbers"
        } else {
            ""
        }
    );
    let _ = writeln!(s, "\n  results: {}", run.dir.display());
    s
}

fn scenario_table(s: &mut String, run: &Run, style: Style) {
    let slo = run.summary.slo_p99_ms;
    let headers = [
        "scenario", "offered", "requests", "rps", "p50", "p90", "p95", "p99", "p99.9", "errors",
        "dropped", "SLO",
    ];
    let mut stats: Vec<&ScenarioStats> = run.stats.iter().collect();
    stats.sort_by(|a, b| {
        order(&a.name)
            .cmp(&order(&b.name))
            .then(a.offered_rps.total_cmp(&b.offered_rps))
    });
    let rows: Vec<(Vec<String>, bool)> = stats
        .iter()
        .map(|st| {
            let lat = |k: &str| st.latency_ms.get(k).map_or("-".into(), |v| fmt_ms(*v));
            (
                vec![
                    st.name.clone(),
                    format!("{:.0}", st.offered_rps),
                    format!("{:.0}", st.requests),
                    format!("{:.1}", st.rps),
                    lat("p50"),
                    lat("p90"),
                    lat("p95"),
                    lat("p99"),
                    lat("p99.9"),
                    format!("{:.2}%", st.error_rate * 100.0),
                    format!("{:.0}", st.dropped),
                ],
                st.sustained(slo),
            )
        })
        .collect();

    if style == Style::Markdown {
        let _ = writeln!(s, "| {} |", headers.join(" | "));
        let _ = writeln!(s, "|{}", "---|".repeat(headers.len()));
        for (cells, ok) in rows {
            let _ = writeln!(s, "| {} | {} |", cells.join(" | "), style.ok(ok));
        }
        return;
    }
    let widths = [12, 8, 9, 9, 9, 9, 9, 9, 9, 7, 7, 3];
    let mut line = String::new();
    for (h, w) in headers.iter().zip(widths) {
        let _ = write!(line, "{h:>w$} ");
    }
    let _ = writeln!(s, "{}", style.paint("2", line.trim_end()));
    for (cells, ok) in rows {
        let mut line = String::new();
        for (c, w) in cells.iter().zip(widths) {
            let _ = write!(line, "{c:>w$} ");
        }
        let _ = writeln!(s, "{} {}", line.trim_end(), style.ok(ok));
    }
    let _ = writeln!(
        s,
        "  SLO: p99 ≤ {slo} ms, errors < 1%, k6 dropped < 0.1% of iterations"
    );
}

/// Table order: the measured window first, then spike phases in time order.
fn order(name: &str) -> u8 {
    match name {
        "measure" => 0,
        "spike_pre" => 1,
        "spike_peak" => 2,
        "spike_post" => 3,
        _ => 4,
    }
}

fn fmt_ms(ms: f64) -> String {
    if ms < 1.0 {
        format!("{:.0}µs", ms * 1000.0)
    } else if ms < 100.0 {
        format!("{ms:.2}ms")
    } else {
        format!("{ms:.0}ms")
    }
}

fn fmt_value(m: &Metric) -> String {
    match m.unit {
        "ms" => fmt_ms(m.value),
        "ratio" => format!("{:.3}%", m.value * 100.0),
        "rps" => format!("{:.1} rps", m.value),
        unit => format!("{:.2} {unit}", m.value),
    }
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
    thresholds_passed: bool,
    generator_saturated: bool,
    wash_image: &'a str,
}

fn jsonl(run: &Run) -> Result<()> {
    let meta = Meta::capture()?;
    let param = run.param();
    let thresholds_passed = run.thresholds_passed();
    let generator_saturated = run.generator_saturated();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for m in run.metrics() {
        let row = Row {
            bench: "k6",
            group: &run.summary.scenario,
            param: &param,
            meta: &meta,
            metric: &m.name,
            value: m.value,
            unit: m.unit,
            better: m.better.as_str(),
            thresholds_passed,
            generator_saturated,
            wash_image: &run.meta.wash_image,
        };
        serde_json::to_writer(&mut out, &row)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

fn delta(a: &Run, b: &Run, style: Style) -> String {
    let mut s = String::new();
    let title = format!(
        "k6 delta — {} · {}   {}  →  {}",
        a.summary.scenario,
        a.param(),
        a.dir.file_name().unwrap_or_default().to_string_lossy(),
        b.dir.file_name().unwrap_or_default().to_string_lossy(),
    );
    if style == Style::Markdown {
        let _ = writeln!(s, "### {title}\n");
        let _ = writeln!(s, "| metric | baseline | candidate | Δ | |");
        let _ = writeln!(s, "|---|---|---|---|---|");
    } else {
        let _ = writeln!(s, "\n{}", style.paint("1", &title));
    }

    let bm = b.metrics();
    for ma in a.metrics() {
        let Some(mb) = bm.iter().find(|m| m.name == ma.name) else {
            continue;
        };
        let pct = if ma.value == 0.0 {
            0.0
        } else {
            (mb.value - ma.value) / ma.value * 100.0
        };
        let improved = match ma.better {
            Better::Higher => pct > 0.0,
            Better::Lower => pct < 0.0,
        };
        let (mark, code) = if pct.abs() < NOISE_PCT {
            ("≈ noise", "2")
        } else if improved {
            ("▲ improved", "32")
        } else {
            ("▼ regressed", "31")
        };
        let _ = match style {
            Style::Markdown => writeln!(
                s,
                "| `{}` | {} | {} | {pct:+.1}% | {mark} |",
                ma.name,
                fmt_value(&ma),
                fmt_value(mb)
            ),
            _ => writeln!(
                s,
                "  {:<26} {:>14} → {:<14} {:>+7.1}%  {}",
                ma.name,
                fmt_value(&ma),
                fmt_value(mb),
                pct,
                style.paint(code, mark)
            ),
        };
    }
    for (label, run) in [("baseline", a), ("candidate", b)] {
        if run.generator_saturated() {
            let _ = writeln!(
                s,
                "\n  warning: {label} was generator-saturated; its numbers are k6's ceiling"
            );
        }
    }
    s
}
