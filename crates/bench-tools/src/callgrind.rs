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

/// Whether `bench` is an instruction-count bench rather than a wall-clock one.
///
/// Every gungraun-harness bench target is named `gungraun*` (`gungraun`,
/// `gungraun_plugin`, ...) and writes to `$CARGO_TARGET_DIR/gungraun/` instead
/// of `criterion/`. Matching the prefix rather than a fixed set means a new
/// gungraun bench is classified correctly the moment it is added — an exact
/// match silently routes it down the criterion path, where it finds no output
/// and reports nothing. `scripts/bench/run-bench.sh` and `compare-bench.sh`
/// key off the same prefix.
pub fn is_instruction_bench(bench: &str) -> bool {
    bench.starts_with("gungraun")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn instruction_column_follows_events_and_uses_the_last_summary() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("callgrind.out");
        fs::write(
            &path,
            "version: 1\nevents: Dr Ir Dw\nsummary: 9 123 8\nsummary: 10 456 20\n",
        )?;
        assert_eq!(read_ir(&path)?, Some(456));
        fs::write(&path, "events: Ir\nsummary: 0\n")?;
        assert_eq!(read_ir(&path)?, Some(0));
        Ok(())
    }

    #[test]
    fn incomplete_or_unrelated_event_sets_have_no_instruction_count() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("callgrind.out");
        for data in [
            "",
            "events: Ir\n",
            "summary: 7\n",
            "events: Dr Dw\nsummary: 1 2\n",
            "events: Dr Ir\nsummary: 1\n",
        ] {
            fs::write(&path, data)?;
            assert_eq!(read_ir(&path)?, None, "input: {data:?}");
        }
        Ok(())
    }

    #[test]
    fn malformed_and_missing_files_fail_with_path_context() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("callgrind.out");
        assert!(
            read_ir(&path)
                .expect_err("missing output must fail")
                .to_string()
                .contains("callgrind.out")
        );
        for count in ["invalid", "-1", "18446744073709551616"] {
            fs::write(&path, format!("events: Dr Ir\nsummary: 1 {count}\n"))?;
            let error = read_ir(&path).expect_err("invalid instruction count must fail");
            assert!(error.to_string().contains("summary[1]"));
            assert!(error.to_string().contains("callgrind.out"));
        }
        fs::write(&path, [0xff])?;
        assert!(
            read_ir(&path)
                .expect_err("invalid UTF-8 must fail")
                .to_string()
                .contains("read ")
        );
        Ok(())
    }

    #[test]
    fn walker_sums_threads_and_parts_and_ignores_unrelated_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        for (relative, text) in [
            (
                "pkg/target/beta/p2/callgrind.p2.t1.p1.out",
                "events: Ir\nsummary: 20\n",
            ),
            (
                "pkg/target/beta/p2/callgrind.p2.t2.p1.out",
                "events: Ir\nsummary: 0\n",
            ),
            (
                "pkg/target/beta/p2/callgrind.p2.t1.p2.out",
                "events: Ir\nsummary: 30\n",
            ),
            (
                "pkg/target/alpha/p3/callgrind.out",
                "events: Dr Ir\nsummary: 1 7\n",
            ),
            ("pkg/target/alpha/p3/callgrind.p3.log", "not an output file"),
            (
                "pkg/target/alpha/p3/callgrind.incomplete.out",
                "events: Dr\nsummary: 4\n",
            ),
            ("callgrind.out", "events: Ir\nsummary: 900\n"),
            ("shallow/callgrind.out", "events: Ir\nsummary: 800\n"),
        ] {
            let path = root.path().join(relative);
            fs::create_dir_all(
                path.parent()
                    .ok_or_else(|| anyhow::anyhow!("missing parent"))?,
            )?;
            fs::write(path, text)?;
        }
        let rows = walk(root.path())?;
        assert_eq!(
            rows.iter()
                .map(|r| (r.group.as_str(), r.param.as_str(), r.ir))
                .collect::<Vec<_>>(),
            [("alpha", "p3", 7), ("beta", "p2", 50)]
        );
        assert!(walk(&root.path().join("missing"))?.is_empty());
        assert_eq!(dir_from_target(root.path()), root.path().join("gungraun"));
        Ok(())
    }

    #[test]
    fn names_and_layouts_classify_only_instruction_benchmarks() {
        for name in ["callgrind.out", "callgrind.id.t1.p2.out"] {
            assert!(is_callgrind_out(name.as_ref()));
        }
        for name in [
            "callgrind",
            "other.out",
            "callgrind.out.old",
            "callgrind.log",
        ] {
            assert!(!is_callgrind_out(name.as_ref()));
        }
        assert!(is_instruction_bench("gungraun"));
        assert!(is_instruction_bench("gungraun_plugin"));
        assert!(!is_instruction_bench("http_invoke"));
        assert_eq!(
            group_param_from_dir(Path::new("root"), Path::new("root/pkg/bench/group/param")),
            Some(("group".into(), "param".into()))
        );
        assert_eq!(
            group_param_from_dir(Path::new("root"), Path::new("outside/group/param")),
            None
        );
        assert_eq!(
            group_param_from_dir(Path::new("root"), Path::new("root/group")),
            None
        );
    }
}
