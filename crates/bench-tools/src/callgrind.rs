//! Parser for valgrind's `callgrind.out` output — pulls instruction counts
//! out of the trailing `events:` / `summary:` lines.
//!
//! gungraun invokes valgrind
//! with `--tool=callgrind`, which writes per bench a tree under
//! `target/gungraun/`:
//!
//! ```text
//! target/gungraun/
//!   <package>/<bench_target>/<group>/<param>/
//!     callgrind.<param>.t<thread>.p<part>.out   (one per thread/part)
//!     callgrind.<param>.log
//! ```
//!
//! Older gungraun/iai-callgrind versions wrote a single `callgrind.out` per
//! bench dir; current versions split it into one file per `(thread, part)`,
//! e.g. `callgrind.cold_invocation.p2.t1.p1.out`. Idle secondary threads
//! still emit a file whose `summary:` is `0`, so a bench's true `Ir` is the
//! **sum** across all of its files. We match both layouts (`callgrind.*.out`)
//! and sum per bench directory — see [`walk`].
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

use std::collections::BTreeMap;
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

/// Walk `gungraun_dir`, returning one [`Row`] per bench `(group, param)`.
///
/// A bench emits several callgrind output files — one per `(thread, part)`,
/// named `callgrind.<id>.t<thread>.p<part>.out` (older versions wrote a
/// single `callgrind.out`). A bench's `Ir` is the **sum** across all of its
/// files, since idle secondary threads still write a `summary: 0` file. We
/// therefore group by the containing directory — which uniquely identifies
/// one `(group, param)` bench — and sum.
///
/// Every accepted file is logged to stderr as
/// `bench-tools callgrind: <path> Ir=<ir>`, and every emitted bench as
/// `bench-tools callgrind: (<group>, <param>) Ir=<ir>`, so an operator can
/// audit exactly which files contributed — because we derive `(group, param)`
/// from path components, a stray `callgrind.*.out` in the tree (e.g. leftover
/// from manual debugging) would otherwise silently appear as a real row.
pub fn walk(gungraun_dir: &Path) -> Result<Vec<Row>> {
    if !gungraun_dir.exists() {
        return Ok(Vec::new());
    }
    // Sum Ir per containing directory. BTreeMap gives a deterministic,
    // path-sorted output order.
    let mut totals: BTreeMap<PathBuf, u64> = BTreeMap::new();
    for entry in WalkDir::new(gungraun_dir).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        if !is_callgrind_out(entry.file_name()) {
            continue;
        }
        let Some(ir) = read_ir(entry.path())? else {
            continue;
        };
        let Some(dir) = entry.path().parent() else {
            continue;
        };
        eprintln!("bench-tools callgrind: {} Ir={ir}", entry.path().display());
        *totals.entry(dir.to_path_buf()).or_insert(0) += ir;
    }

    let mut rows = Vec::new();
    for (dir, ir) in totals {
        let Some((group, param)) = group_param_from_dir(gungraun_dir, &dir) else {
            continue;
        };
        eprintln!("bench-tools callgrind: ({group}, {param}) Ir={ir}");
        rows.push(Row { group, param, ir });
    }
    Ok(rows)
}

/// gungraun output files are named `callgrind.out` (old layout) or
/// `callgrind.<id>.t<thread>.p<part>.out` (current). Match both: any file
/// whose name starts with `callgrind.` and ends with `.out`.
fn is_callgrind_out(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|n| n.starts_with("callgrind.") && n.ends_with(".out"))
}

/// Derive `(group, param)` from a bench's directory: the last two path
/// components relative to `gungraun_dir` (`…/<group>/<param>`). `None` if the
/// directory sits fewer than two levels below `gungraun_dir`. Robust against
/// gungraun nesting extra levels above `<group>` (e.g. `<package>/<binary>/…`).
fn group_param_from_dir(gungraun_dir: &Path, dir: &Path) -> Option<(String, String)> {
    let rel = dir.strip_prefix(gungraun_dir).ok()?;
    let components: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    // Last two components are `<group>/<param>`; `None` if the directory sits
    // fewer than two levels below `gungraun_dir`.
    let mut tail = components.iter().rev();
    let param = tail.next()?;
    let group = tail.next()?;
    Some(((*group).to_string(), (*param).to_string()))
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
