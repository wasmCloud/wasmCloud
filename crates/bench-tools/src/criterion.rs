//! Parsers + directory walkers for criterion's per-bench output layout.
//!
//! Criterion writes one tree per `(group, param)` under `target/criterion/`:
//!
//! ```text
//! target/criterion/
//!   <group>/
//!     <param>/
//!       new/                      ← this run
//!         estimates.json          ← point estimates + CI for mean/median/…
//!         benchmark.json          ← config: function_id, value_str, throughput
//!       base/                     ← previous run (we ignore)
//!   report/                       ← rendered HTML (we ignore)
//! ```
//!
//! We only consume `new/estimates.json` + `new/benchmark.json`. The trend
//! pipeline (jsonl subcommand) attributes every row it finds to the current
//! bench name; to keep stale data from prior runs of *other* benches out of
//! the output, callers pass a marker file written by run-bench.sh at the
//! start of the current run and we filter by mtime > marker.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

/// One `<group>/<param>/new/estimates.json` + its sibling `benchmark.json`.
#[derive(Debug)]
pub struct Row {
    pub group: String,
    pub param: String,
    pub estimates: Estimates,
    pub throughput: Option<Throughput>,
}

#[derive(Debug, Deserialize)]
pub struct Estimates {
    pub mean: Estimate,
    pub median: Estimate,
    pub std_dev: Estimate,
}

#[derive(Debug, Deserialize)]
pub struct Estimate {
    pub point_estimate: f64,
    pub confidence_interval: ConfidenceInterval,
}

#[derive(Debug, Deserialize)]
pub struct ConfidenceInterval {
    pub lower_bound: f64,
    pub upper_bound: f64,
}

/// Criterion's `Throughput` enum — kept untagged so JSON looks like
/// `{"Elements": 256}` / `{"Bytes": 1024}` / `null`, matching the trend
/// schema downstream consumers already parse.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Throughput {
    Bytes(u64),
    BytesDecimal(u64),
    Elements(u64),
}

#[derive(Debug, Deserialize)]
struct BenchmarkJson {
    #[serde(default)]
    throughput: Option<Throughput>,
}

/// Walk `criterion_dir`, returning one [`Row`] per `(group, param)` tuple.
///
/// If `marker` is `Some`, only `estimates.json` files modified after the
/// marker's mtime are returned — see module doc-comment for why. If
/// `marker` is `Some` but the marker file is missing, returns an empty
/// vec (defensive: don't risk publishing stale rows).
pub fn walk(criterion_dir: &Path, marker: Option<&Path>) -> Result<Vec<Row>> {
    if !criterion_dir.exists() {
        return Ok(Vec::new());
    }

    let marker_mtime = match marker {
        Some(m) if m.exists() => Some(
            std::fs::metadata(m)
                .with_context(|| format!("stat {}", m.display()))?
                .modified()
                .with_context(|| format!("mtime {}", m.display()))?,
        ),
        Some(m) => {
            eprintln!(
                "bench-tools: no marker at {}; emitting nothing",
                m.display()
            );
            return Ok(Vec::new());
        }
        None => None,
    };

    let mut rows = Vec::new();

    // depth 4 from criterion_dir: <group>/<param>/new/estimates.json
    for entry in WalkDir::new(criterion_dir)
        .min_depth(4)
        .max_depth(4)
        .sort_by_file_name()
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.file_name() != "estimates.json" {
            continue;
        }
        let path = entry.path();
        // Filter to */new/* — base/ is criterion's previous-run baseline.
        let Some(parent) = path.parent() else {
            continue;
        };
        if parent.file_name() != Some(std::ffi::OsStr::new("new")) {
            continue;
        }

        if let Some(want_after) = marker_mtime {
            let mtime = entry
                .metadata()
                .with_context(|| format!("metadata for {}", path.display()))?
                .modified()
                .with_context(|| format!("mtime for {}", path.display()))?;
            if mtime <= want_after {
                continue;
            }
        }

        let (group, param) = derive_group_param(criterion_dir, path)?;
        // Skip criterion's own report dir aggregations (group == "report").
        if group == "report" {
            continue;
        }

        let estimates = parse_estimates(path)?;
        let throughput = parse_benchmark_throughput(parent.join("benchmark.json").as_path())?;

        rows.push(Row {
            group,
            param,
            estimates,
            throughput,
        });
    }

    // Loud warning: criterion dir exists but produced no rows. Silent
    // emptiness is the worst failure mode for trend ingestion — a future
    // criterion layout change (e.g. renaming `new/`) would otherwise
    // surface as "history.json stopped growing" weeks later.
    if rows.is_empty() {
        eprintln!(
            "bench-tools criterion: walked {} but found no <group>/<param>/new/estimates.json entries — \
             criterion layout may have changed, or the marker filter excluded everything",
            criterion_dir.display(),
        );
    }

    Ok(rows)
}

/// Pull `(group, param)` out of a path of shape
/// `<crit>/<group>/<param>/new/estimates.json`.
fn derive_group_param(crit: &Path, estimates_path: &Path) -> Result<(String, String)> {
    let rel = estimates_path
        .strip_prefix(crit)
        .with_context(|| format!("strip_prefix({})", crit.display()))?;
    let components: Vec<_> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    if components.len() < 2 {
        anyhow::bail!("unexpected path shape under criterion/: {}", rel.display());
    }
    let group = components
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("missing group in {}", rel.display()))?
        .to_string();
    let param = components
        .get(1)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("missing param in {}", rel.display()))?
        .to_string();
    Ok((group, param))
}

fn parse_estimates(path: &Path) -> Result<Estimates> {
    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    serde_json::from_reader(f).with_context(|| format!("parse {}", path.display()))
}

fn parse_benchmark_throughput(path: &Path) -> Result<Option<Throughput>> {
    if !path.exists() {
        return Ok(None);
    }
    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let parsed: BenchmarkJson =
        serde_json::from_reader(f).with_context(|| format!("parse {}", path.display()))?;
    Ok(parsed.throughput)
}

/// Convenience: build `criterion_dir` from `$CARGO_TARGET_DIR`.
pub fn dir_from_target(target_dir: &Path) -> PathBuf {
    target_dir.join("criterion")
}
