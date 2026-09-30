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
            ("k6", "-k6"),
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

    let am = a.metrics();
    let bm = b.metrics();
    // Every metric either run has: one missing from the candidate is itself a
    // result (a stress run that sustained nothing has no max_sustainable_rps).
    let mut names: Vec<&str> = am.iter().map(|m| m.name.as_str()).collect();
    for m in &bm {
        if !names.contains(&m.name.as_str()) {
            names.push(&m.name);
        }
    }
    for name in names {
        let ma = am.iter().find(|m| m.name == name);
        let mb = bm.iter().find(|m| m.name == name);
        let row = compare(ma, mb);
        let shown = |m: Option<&Metric>| m.map_or_else(|| "—".to_string(), fmt_value);
        let pct = row
            .pct
            .map_or_else(|| "n/a".to_string(), |p| format!("{p:+.1}%"));
        let _ = match style {
            Style::Markdown => writeln!(
                s,
                "| `{name}` | {} | {} | {pct} | {} |",
                shown(ma),
                shown(mb),
                row.mark
            ),
            _ => writeln!(
                s,
                "  {:<26} {:>14} → {:<14} {:>8}  {}",
                name,
                shown(ma),
                shown(mb),
                pct,
                style.paint(row.code, row.mark)
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

/// One delta line's verdict.
struct Verdict {
    /// Percent change, or `None` where there is none to give: a metric only
    /// one run has, or a move from zero.
    pct: Option<f64>,
    mark: &'static str,
    code: &'static str,
}

fn compare(a: Option<&Metric>, b: Option<&Metric>) -> Verdict {
    let row = |pct, mark, code| Verdict { pct, mark, code };
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        (Some(_), None) => return row(None, "▼ missing in candidate", "31"),
        _ => return row(None, "new in candidate", "2"),
    };
    let pct = if a.value == 0.0 {
        (b.value == 0.0).then_some(0.0)
    } else {
        Some((b.value - a.value) / a.value * 100.0)
    };
    if pct.is_some_and(|p| p.abs() < NOISE_PCT) {
        return row(pct, "≈ noise", "2");
    }
    let improved = match a.better {
        Better::Higher => b.value > a.value,
        Better::Lower => b.value < a.value,
    };
    if improved {
        row(pct, "▲ improved", "32")
    } else {
        row(pct, "▼ regressed", "31")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metric(value: f64, better: Better) -> Metric {
        Metric {
            name: "m".to_string(),
            value,
            unit: "ratio",
            better,
        }
    }

    #[test]
    fn a_move_from_zero_is_not_noise() {
        let row = compare(
            Some(&metric(0.0, Better::Lower)),
            Some(&metric(0.05, Better::Lower)),
        );
        assert_eq!(row.pct, None);
        assert_eq!(row.mark, "▼ regressed");

        let still = compare(
            Some(&metric(0.0, Better::Lower)),
            Some(&metric(0.0, Better::Lower)),
        );
        assert_eq!(still.mark, "≈ noise");
    }

    #[test]
    fn a_metric_missing_from_the_candidate_is_a_regression() {
        let row = compare(Some(&metric(8000.0, Better::Higher)), None);
        assert_eq!(row.mark, "▼ missing in candidate");
        assert_eq!(
            compare(None, Some(&metric(1.0, Better::Higher))).mark,
            "new in candidate"
        );
    }

    #[test]
    fn small_moves_are_noise_and_direction_follows_better() {
        let up = compare(
            Some(&metric(100.0, Better::Higher)),
            Some(&metric(110.0, Better::Higher)),
        );
        assert_eq!(up.mark, "▲ improved");
        let slower = compare(
            Some(&metric(10.0, Better::Lower)),
            Some(&metric(11.0, Better::Lower)),
        );
        assert_eq!(slower.mark, "▼ regressed");
        let noise = compare(
            Some(&metric(100.0, Better::Higher)),
            Some(&metric(101.0, Better::Higher)),
        );
        assert_eq!(noise.mark, "≈ noise");
    }
}
