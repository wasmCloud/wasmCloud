//! `bench-tools delta` — render a markdown delta table from two snapshotted
//! runs produced by `compare-bench.sh`.
//!
//! Expects the snapshot layout that `compare-bench.sh` writes:
//!
//! ```text
//! $WASMCLOUD_BENCH_COMPARE_DIR/
//!   a/iter-1/criterion/ …
//!   a/iter-1/iai/       …
//!   a/iter-2/…          (when criterion benches run 3× interleaved)
//!   b/iter-1/criterion/ …
//!   b/iter-1/iai/       …
//! ```
//!
//! For each side we collect one numeric value per `(group, param)` per
//! iteration (criterion → `mean_ns`; iai → `Ir`), take the median across
//! iterations, then compute `Δ = (B − A) / A · 100`. Output is markdown
//! plus a sibling `$WASMCLOUD_BENCH_COMPARE_DIR/delta.md` file that the
//! bench-compare workflow appends to `$GITHUB_STEP_SUMMARY`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::callgrind;
use crate::criterion;
use crate::meta::Meta;

#[derive(Debug, clap::Args)]
pub struct Args {
    #[arg(long, env = "WASMCLOUD_BENCH_COMPARE_DIR")]
    pub compare_dir: PathBuf,
    #[arg(long, env = "WASMCLOUD_BENCH_NAME")]
    pub bench: String,
    #[arg(long, env = "WASMCLOUD_BENCH_REF_A")]
    pub ref_a: String,
    #[arg(long, env = "WASMCLOUD_BENCH_REF_B")]
    pub ref_b: String,
    #[arg(long, env = "WASMCLOUD_BENCH_SHORT_A")]
    pub short_a: String,
    #[arg(long, env = "WASMCLOUD_BENCH_SHORT_B")]
    pub short_b: String,
    #[arg(long, env = "WASMCLOUD_BENCH_ITERS")]
    pub iters: u32,
}

/// `(group, param) → [values across iterations]`
type Series = BTreeMap<(String, String), Vec<f64>>;

pub fn run(args: Args) -> Result<()> {
    let kind = Kind::for_bench(&args.bench);

    // One capture for the whole render — host/kernel/timestamp are constant
    // across the run, and Meta::capture() forks ~5 subprocesses we don't
    // want to repeat per call site.
    let meta = Meta::capture()?;

    let a = collect_side(&args.compare_dir.join("a"), kind)?;
    let b = collect_side(&args.compare_dir.join("b"), kind)?;

    let body = render(&args, kind, &meta, &a, &b);

    let out_path = args.compare_dir.join("delta.md");
    std::fs::write(&out_path, &body).with_context(|| format!("write {}", out_path.display()))?;

    print!("{body}");
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    /// criterion mean_ns; "lower is better".
    Time,
    /// iai-callgrind Ir; "lower is better".
    Instructions,
}

impl Kind {
    fn for_bench(bench: &str) -> Self {
        match bench {
            "iai_callgrind" => Kind::Instructions,
            _ => Kind::Time,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Time => "mean time",
            Kind::Instructions => "Ir (instructions)",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Kind::Time => "ns",
            Kind::Instructions => "instr",
        }
    }
}

fn collect_side(side_dir: &Path, kind: Kind) -> Result<Series> {
    let mut series: Series = BTreeMap::new();
    if !side_dir.exists() {
        return Ok(series);
    }
    for entry in
        std::fs::read_dir(side_dir).with_context(|| format!("read_dir {}", side_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let iter_dir = entry.path();
        let name = entry.file_name();
        // Only look at `iter-*` children.
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("iter-") {
            continue;
        }
        match kind {
            Kind::Time => collect_criterion(&iter_dir, &mut series)?,
            Kind::Instructions => collect_iai(&iter_dir, &mut series)?,
        }
    }
    Ok(series)
}

fn collect_criterion(iter_dir: &Path, series: &mut Series) -> Result<()> {
    let crit_dir = iter_dir.join("criterion");
    // No marker filter when reading snapshots — they're already isolated per
    // iteration on disk.
    let rows = criterion::walk(&crit_dir, None)?;
    for row in rows {
        series
            .entry((row.group, row.param))
            .or_default()
            .push(row.estimates.mean.point_estimate);
    }
    Ok(())
}

fn collect_iai(iter_dir: &Path, series: &mut Series) -> Result<()> {
    let iai_dir = iter_dir.join("iai");
    let rows = callgrind::walk(&iai_dir)?;
    for row in rows {
        series
            .entry((row.group, row.param))
            .or_default()
            .push(row.ir as f64);
    }
    Ok(())
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    // Matches the jq implementation we replaced: pick the upper-middle for
    // even-length series. Single-iteration runs (iai) hit the mid=0 path.
    sorted.get(mid).copied()
}

fn render(args: &Args, kind: Kind, meta: &Meta, a: &Series, b: &Series) -> String {
    use std::collections::BTreeSet;
    use std::fmt::Write;

    let mut keys: BTreeSet<&(String, String)> = BTreeSet::new();
    keys.extend(a.keys());
    keys.extend(b.keys());

    let mut out = String::new();
    let _ = writeln!(out, "## bench-compare: `{}`", args.bench);
    let _ = writeln!(out);
    let _ = writeln!(out, "| | |");
    let _ = writeln!(out, "|---|---|");
    let _ = writeln!(
        out,
        "| **A (baseline)** | `{}` @ `{}` |",
        args.ref_a, args.short_a
    );
    let _ = writeln!(
        out,
        "| **B (candidate)** | `{}` @ `{}` |",
        args.ref_b, args.short_b
    );
    let _ = writeln!(out, "| **iters per side** | {} (median used) |", args.iters);
    let _ = writeln!(
        out,
        "| **metric** | {} ({}) — lower is better |",
        kind.label(),
        kind.unit()
    );
    let _ = writeln!(
        out,
        "| **host** | `{}` · {} cpu · isolated cpu: `{}` |",
        meta.host, meta.cpus_online, meta.isolated_cpu,
    );
    let _ = writeln!(out, "| **timestamp** | {} |", meta.timestamp);
    let _ = writeln!(out);

    if keys.is_empty() {
        let _ = writeln!(
            out,
            "_no measurement output found at `{}` — was the bench run successfully?_",
            args.compare_dir.display()
        );
        return out;
    }

    let _ = writeln!(out, "| group | param | A | B | Δ |");
    let _ = writeln!(out, "|---|---|---:|---:|---:|");
    for key in keys {
        let am = a.get(key).and_then(|v| median(v));
        let bm = b.get(key).and_then(|v| median(v));
        let delta = match (am, bm) {
            (Some(av), Some(bv)) if av != 0.0 => Some((bv - av) / av * 100.0),
            _ => None,
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            key.0,
            key.1,
            fmt_value(am, kind),
            fmt_value(bm, kind),
            fmt_delta(delta),
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "_Δ is `(B − A) / A · 100` of the median across {} iteration(s). \
         Single-iteration comparisons (`iai_callgrind`) are deterministic; \
         multi-iteration comparisons use the median to dampen run-to-run noise._",
        args.iters
    );
    out
}

fn fmt_value(v: Option<f64>, kind: Kind) -> String {
    match (v, kind) {
        (Some(v), Kind::Time) => format!("{v:.0}"),
        (Some(v), Kind::Instructions) => format!("{}", v as u64),
        (None, _) => "—".to_string(),
    }
}

fn fmt_delta(v: Option<f64>) -> String {
    match v {
        Some(pct) => format!("{:.2}%", pct),
        None => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_single() {
        assert_eq!(median(&[100.0]), Some(100.0));
    }

    #[test]
    fn median_odd() {
        assert_eq!(median(&[99.0, 101.0, 100.0]), Some(100.0));
    }

    #[test]
    fn median_even_picks_upper_mid() {
        // `length / 2 | floor` semantics — for [1,2,3,4] the median is
        // the 3rd element. Documented so a future refactor doesn't
        // "fix" this into the arithmetic mean of the two middle values.
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(3.0));
    }

    #[test]
    fn median_empty() {
        assert_eq!(median(&[]), None);
    }
}
