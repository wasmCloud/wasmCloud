//! Parser for valgrind's `callgrind.out` output — pulls instruction counts
//! out of the trailing `events:` / `summary:` lines.
//!
//! gungraun invokes valgrind
//! with `--tool=callgrind`, which writes per bench a tree under
//! `target/gungraun/`:
//!
//! ```text
//! target/gungraun/
//!   <package>/<bench_target>/<function>.<id>/
//!     callgrind.out
//!     summary.json     (we don't use this — schema changes across versions)
//!     callgrind.out.old (baseline if any)
//! ```
//!
//! The file format itself is callgrind's, set by valgrind, and is stable
//! across the gungraun rename — so this parser's name stays `callgrind`
//! rather than tracking the upstream crate.
//!
//! callgrind.out itself ends with two lines whose format is set by valgrind
//! and very stable:
//!
//! ```text
//! events: Ir Dr Dw I1mr D1mr ILmr DLmr
//! summary: 12345 6789 1234 ...
//! ```
//!
//! We read those two lines, look up the column index of `Ir`, and pull the
//! corresponding number out of `summary:`. That's the metric we report —
//! instruction reads, i.e. total instructions executed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

#[derive(Debug)]
pub struct Row {
    /// `<bench_target>` — the `library_benchmark_group!` name.
    pub group: String,
    /// `<function>.<id>` — the per-bench identity.
    pub param: String,
    /// Instruction count (`Ir`).
    pub ir: u64,
}

/// Walk `gungraun_dir`, returning one [`Row`] per `callgrind.out` found.
///
/// Every accepted file is logged to stderr in the form
/// `bench-tools callgrind: <path> → (<group>, <param>) Ir=<ir>` so an
/// operator can audit exactly which files contributed to the output —
/// because we derive `(group, param)` from path components, a stray
/// callgrind.out in the tree (e.g. leftover from manual debugging) would
/// otherwise silently appear as a real bench row.
pub fn walk(gungraun_dir: &Path) -> Result<Vec<Row>> {
    if !gungraun_dir.exists() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in WalkDir::new(gungraun_dir).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.file_name() != "callgrind.out" {
            continue;
        }
        let Some(row) = parse_one(gungraun_dir, entry.path())? else {
            continue;
        };
        eprintln!(
            "bench-tools callgrind: {} → ({}, {}) Ir={}",
            entry.path().display(),
            row.group,
            row.param,
            row.ir,
        );
        rows.push(row);
    }
    Ok(rows)
}

fn parse_one(gungraun_dir: &Path, path: &Path) -> Result<Option<Row>> {
    let rel = path
        .strip_prefix(gungraun_dir)
        .with_context(|| format!("strip_prefix({})", gungraun_dir.display()))?;

    // `<package>/<group>/<param>/callgrind.out`
    // We pick the last two non-file components as (group, param). This is
    // robust against gungraun nesting one or more extra levels above
    // <group>, which it has done in some versions (e.g. <package>/<binary>/…).
    let components: Vec<_> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    if components.len() < 3 {
        return Ok(None);
    }
    let len = components.len();
    let Some(group) = components.get(len - 3) else {
        return Ok(None);
    };
    let Some(param) = components.get(len - 2) else {
        return Ok(None);
    };

    let ir = match read_ir(path)? {
        Some(v) => v,
        None => return Ok(None),
    };
    Ok(Some(Row {
        group: (*group).to_string(),
        param: (*param).to_string(),
        ir,
    }))
}

/// Return the `Ir` column of the `summary:` line, looked up by index in
/// the `events:` line. `None` if either line is missing or `Ir` isn't
/// among the events (callgrind was invoked with a different event set).
pub fn read_ir(path: &Path) -> Result<Option<u64>> {
    use std::io::{BufRead, BufReader};

    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut events: Option<Vec<String>> = None;
    let mut summary: Option<Vec<String>> = None;
    for line in BufReader::new(f).lines() {
        let line = line.with_context(|| format!("read {}", path.display()))?;
        if let Some(rest) = line.strip_prefix("events: ") {
            events = Some(rest.split_whitespace().map(String::from).collect());
        } else if let Some(rest) = line.strip_prefix("summary: ") {
            summary = Some(rest.split_whitespace().map(String::from).collect());
        }
    }
    let (Some(events), Some(summary)) = (events, summary) else {
        return Ok(None);
    };
    let Some(idx) = events.iter().position(|e| e == "Ir") else {
        return Ok(None);
    };
    let Some(s) = summary.get(idx) else {
        return Ok(None);
    };
    Ok(Some(s.parse().with_context(|| {
        format!("summary[{idx}] = {s:?} in {}", path.display())
    })?))
}

pub fn dir_from_target(target_dir: &Path) -> PathBuf {
    target_dir.join("gungraun")
}
