//! `bench-tools jsonl` emit one JSON row per `(group, param, metric)`
//!
//! Schema (additive, existing consumers keep working):
//!
//! - Every row has `metric` (`"mean_ns"` or `"Ir"`) and `value: f64`.
//! - Criterion rows additionally carry the original sibling fields
//!   (`mean_ns`, `median_ns`, `std_dev_ns`, `ci_low_ns`, `ci_high_ns`,
//!   `throughput`) so the existing `arewefastyet` reader doesn't break
//!   on the schema bump.
//! - iai rows carry only `metric` + `value` (Ir instruction counts).
//!
//! The dedup key in `push-s3.sh` is widened to include `.metric`, which
//! lets `(bench, group, param)` map to multiple rows, one per metric,
//! without each colliding with the others.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::callgrind;
use crate::criterion::{self, Throughput};
use crate::meta::Meta;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Bench name (e.g. `http_invoke`); annotates every emitted row.
    #[arg(long)]
    pub bench: String,

    /// `$CARGO_TARGET_DIR`. Defaults to env var, then `target/`.
    #[arg(long, env = "CARGO_TARGET_DIR")]
    pub target_dir: Option<PathBuf>,

    /// Marker file written by run-bench.sh at the start of the current
    /// run. Used only for criterion: prevents stale rows from prior runs
    /// of other benches leaking out. Defaults to
    /// `$CARGO_TARGET_DIR/.bench-start-<bench>`.
    #[arg(long)]
    pub marker: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct CriterionRow<'a> {
    bench: &'a str,
    group: String,
    param: String,
    #[serde(flatten)]
    meta: &'a Meta,
    metric: &'static str,
    value: f64,
    throughput: Option<Throughput>,
    mean_ns: f64,
    median_ns: f64,
    std_dev_ns: f64,
    ci_low_ns: f64,
    ci_high_ns: f64,
}

#[derive(Debug, Serialize)]
struct IaiRow<'a> {
    bench: &'a str,
    group: String,
    param: String,
    #[serde(flatten)]
    meta: &'a Meta,
    metric: &'static str,
    value: f64,
}

pub fn run(args: Args) -> Result<()> {
    // Pull non-Copy fields out of `args` first so subsequent `&args.bench`
    // borrows don't conflict with the partial move from `args.target_dir`.
    let target_dir: PathBuf = args
        .target_dir
        .as_deref()
        .unwrap_or(Path::new("target"))
        .to_path_buf();
    let marker_override = args.marker.clone();
    let meta = Meta::capture()?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    if args.bench == "iai_callgrind" {
        return emit_iai(&mut out, &args.bench, &meta, &target_dir);
    }

    emit_criterion(
        &mut out,
        &args.bench,
        marker_override.as_deref(),
        &meta,
        &target_dir,
    )
}

fn emit_criterion<W: Write>(
    out: &mut W,
    bench: &str,
    marker_override: Option<&Path>,
    meta: &Meta,
    target_dir: &Path,
) -> Result<()> {
    let crit_dir = criterion::dir_from_target(target_dir);
    let marker_owned: PathBuf;
    let marker: &Path = match marker_override {
        Some(p) => p,
        None => {
            marker_owned = target_dir.join(format!(".bench-start-{bench}"));
            &marker_owned
        }
    };

    let rows = criterion::walk(&crit_dir, Some(marker))?;
    for row in rows {
        let est = &row.estimates;
        let serialized = CriterionRow {
            bench,
            group: row.group,
            param: row.param,
            meta,
            metric: "mean_ns",
            value: est.mean.point_estimate,
            throughput: row.throughput,
            mean_ns: est.mean.point_estimate,
            median_ns: est.median.point_estimate,
            std_dev_ns: est.std_dev.point_estimate,
            ci_low_ns: est.mean.confidence_interval.lower_bound,
            ci_high_ns: est.mean.confidence_interval.upper_bound,
        };
        serde_json::to_writer(&mut *out, &serialized)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

fn emit_iai<W: Write>(
    out: &mut W,
    bench: &str,
    meta: &Meta,
    target_dir: &std::path::Path,
) -> Result<()> {
    let iai_dir = callgrind::dir_from_target(target_dir);
    let rows = callgrind::walk(&iai_dir)?;
    for row in rows {
        let serialized = IaiRow {
            bench,
            group: row.group,
            param: row.param,
            meta,
            metric: "Ir",
            value: row.ir as f64,
        };
        serde_json::to_writer(&mut *out, &serialized)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}
