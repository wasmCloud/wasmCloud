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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs::{self, FileTimes};
    use std::time::{Duration, SystemTime};

    fn fixture(
        root: &Path,
        group: &str,
        param: &str,
        throughput: Option<Throughput>,
    ) -> Result<PathBuf> {
        let dir = root.join(group).join(param).join("new");
        fs::create_dir_all(&dir)?;
        let estimate = json!({"point_estimate": 1000.0, "confidence_interval": {"lower_bound": 900.0, "upper_bound": 1100.0}});
        let path = dir.join("estimates.json");
        fs::write(
            &path,
            serde_json::to_vec(
                &json!({"mean": estimate, "median": estimate, "std_dev": estimate}),
            )?,
        )?;
        fs::write(
            dir.join("benchmark.json"),
            serde_json::to_vec(&json!({"throughput": throughput}))?,
        )?;
        Ok(path)
    }

    #[test]
    fn walks_only_current_estimates_in_stable_order() -> Result<()> {
        let root = tempfile::tempdir()?;
        fixture(root.path(), "z-group", "p2", Some(Throughput::Elements(8)))?;
        fixture(
            root.path(),
            "a-group",
            "p3",
            Some(Throughput::BytesDecimal(1024)),
        )?;
        let excluded = fixture(root.path(), "report", "aggregate", None)?;
        fs::write(excluded, b"not json")?;
        let base = root.path().join("a-group/p3/base");
        fs::create_dir_all(&base)?;
        fs::write(base.join("estimates.json"), b"not json")?;
        fs::write(root.path().join("estimates.json"), b"not json")?;
        fs::create_dir_all(root.path().join("a-group/empty/new/estimates.json"))?;
        let rows = walk(root.path(), None)?;
        assert_eq!(
            rows.iter()
                .map(|r| (r.group.as_str(), r.param.as_str()))
                .collect::<Vec<_>>(),
            [("a-group", "p3"), ("z-group", "p2")]
        );
        let mut rows = rows.into_iter();
        let first = rows
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing first benchmark"))?;
        assert!(matches!(
            first.throughput,
            Some(Throughput::BytesDecimal(1024))
        ));
        assert_eq!(first.estimates.mean.point_estimate, 1000.0);
        assert_eq!(first.estimates.mean.confidence_interval.lower_bound, 900.0);
        let second = rows
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing second benchmark"))?;
        assert!(matches!(second.throughput, Some(Throughput::Elements(8))));
        Ok(())
    }

    #[test]
    fn marker_excludes_equal_and_older_results_without_parsing_them() -> Result<()> {
        let root = tempfile::tempdir()?;
        let marker = root.path().join("marker");
        fs::write(&marker, [])?;
        let cutoff = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(&marker)?
            .set_times(FileTimes::new().set_modified(cutoff))?;
        for (param, seconds) in [("old", -10), ("equal", 0), ("fresh", 10)] {
            let path = fixture(root.path(), "group", param, None)?;
            if seconds <= 0 {
                fs::write(&path, b"stale malformed output")?;
            }
            let modified = if seconds < 0 {
                cutoff - Duration::from_secs(10)
            } else {
                cutoff + Duration::from_secs(seconds as u64)
            };
            fs::File::options()
                .write(true)
                .open(path)?
                .set_times(FileTimes::new().set_modified(modified))?;
        }
        let rows = walk(root.path(), Some(&marker))?;
        assert_eq!(rows.len(), 1);
        assert!(rows.iter().all(|row| row.param == "fresh"));
        assert!(walk(root.path(), Some(&root.path().join("missing-marker")))?.is_empty());
        Ok(())
    }

    #[test]
    fn throughput_is_optional_but_malformed_metadata_is_an_error() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = fixture(root.path(), "group", "param", None)?;
        let metadata = path.with_file_name("benchmark.json");
        fs::remove_file(&metadata)?;
        assert!(
            walk(root.path(), None)?
                .iter()
                .all(|r| r.throughput.is_none())
        );
        fs::write(&metadata, b"{}")?;
        assert!(
            walk(root.path(), None)?
                .iter()
                .all(|r| r.throughput.is_none())
        );
        fs::write(&metadata, br#"{"throughput":{"Bytes":16}}"#)?;
        assert!(
            walk(root.path(), None)?
                .iter()
                .all(|r| matches!(r.throughput, Some(Throughput::Bytes(16))))
        );
        fs::write(&metadata, b"broken metadata")?;
        let error = walk(root.path(), None).expect_err("invalid benchmark metadata must fail");
        assert!(error.to_string().contains("benchmark.json"));
        fs::remove_file(&metadata)?;
        fs::write(&path, b"broken estimates")?;
        assert!(
            walk(root.path(), None)
                .expect_err("invalid estimates must fail")
                .to_string()
                .contains("estimates.json")
        );
        Ok(())
    }

    #[test]
    fn empty_missing_and_invalid_paths_are_distinct() -> Result<()> {
        let root = tempfile::tempdir()?;
        assert!(walk(root.path(), None)?.is_empty());
        assert!(walk(&root.path().join("missing"), None)?.is_empty());
        assert!(derive_group_param(root.path(), &root.path().join("too-shallow")).is_err());
        assert!(
            derive_group_param(
                root.path(),
                &root
                    .path()
                    .with_extension("outside")
                    .join("group/param/new/estimates.json")
            )
            .is_err()
        );
        assert!(parse_estimates(&root.path().join("missing.json")).is_err());
        assert_eq!(dir_from_target(root.path()), root.path().join("criterion"));
        Ok(())
    }
}
