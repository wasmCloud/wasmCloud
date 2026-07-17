//! Run-level metadata harvested from git, the GitHub Actions environment,
//! and the bench host's sysfs.
//!
//! Centralized so every subcommand that emits structured output (jsonl,
//! summary, delta) produces consistent provenance fields without each one
//! re-deriving them.

use std::process::Command;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;

/// `2026-05-13T12:34:56Z` — matches `date -u +%FT%TZ`, so history.json rows
/// from before this rewrite stay format-compatible.
const TIMESTAMP_FMT: &[FormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub sha: String,
    pub short_sha: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub run_id: String,
    pub run_attempt: String,
    pub timestamp: String,
    pub host: String,
    pub kernel: String,
    pub cpus_online: u32,
    /// Contents of `/sys/devices/system/cpu/isolated`, or `"?"` if the file
    /// is unreadable or empty (e.g., running this off the bench host).
    pub isolated_cpu: String,
}

impl Meta {
    /// Snapshot the current run's identity from git + env + uname + sysfs.
    pub fn capture() -> Result<Self> {
        let sha = git(&["rev-parse", "HEAD"])?;
        let short_sha = git(&["rev-parse", "--short=12", "HEAD"])?;

        // The ref benched. In CI, bench.yml sets WASMCLOUD_BENCH_REF from the
        // single resolved matrix ref (a tag for `release`, the checked-out
        // branch/tag/sha for a dispatch) — one source of truth. For a hand-run
        // `cargo bench`, derive it from git.
        //
        // We deliberately do NOT read GITHUB_REF_NAME: that is the ref the
        // workflow was *dispatched from* (e.g. `main`), not the ref we checked
        // out and benched, so it silently mislabels `ref`-input runs and hides
        // them from the semver-filtered releases view.
        let ref_name = match std::env::var("WASMCLOUD_BENCH_REF") {
            Ok(r) if !r.is_empty() => r,
            _ => local_git_ref()?,
        };

        let run_id = env_or("GITHUB_RUN_ID", "local");
        let run_attempt = env_or("GITHUB_RUN_ATTEMPT", "1");
        let timestamp = OffsetDateTime::now_utc()
            .format(TIMESTAMP_FMT)
            .context("format current UTC timestamp")?;
        let host = hostname()?;
        let kernel = run(&["uname", "-r"])?;
        let cpus_online = num_cpus_online()?;
        let isolated_cpu = read_isolated_cpu();

        Ok(Self {
            sha,
            short_sha,
            ref_name,
            run_id,
            run_attempt,
            timestamp,
            host,
            kernel,
            cpus_online,
            isolated_cpu,
        })
    }
}

fn git(args: &[&str]) -> Result<String> {
    run(std::iter::once("git")
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .as_slice())
}

/// Best-effort human ref for a locally-run bench (when WASMCLOUD_BENCH_REF is
/// unset): an exact tag at HEAD, else the branch name, else the short sha — so
/// a detached checkout (`git checkout v2.5.2`, compare-bench) still labels
/// sensibly instead of the literal "HEAD".
fn local_git_ref() -> Result<String> {
    if let Ok(tag) = git(&["describe", "--tags", "--exact-match", "HEAD"]) {
        return Ok(tag);
    }
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        return git(&["rev-parse", "--short=12", "HEAD"]);
    }
    Ok(branch)
}

fn run(argv: &[&str]) -> Result<String> {
    let (cmd, rest) = argv.split_first().ok_or_else(|| anyhow!("empty argv"))?;
    let out = Command::new(cmd)
        .args(rest)
        .output()
        .with_context(|| format!("spawning {}", argv.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "{} exited {}: {}",
            argv.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8(out.stdout)
        .with_context(|| format!("{} produced non-utf8", argv.join(" ")))?
        .trim()
        .to_string())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn hostname() -> Result<String> {
    // Avoid pulling in a dep for one syscall; libc hostname() reads
    // /proc/sys/kernel/hostname through the kernel on Linux. /etc/hostname
    // is the persistent value and matches what `hostname` (the command)
    // prints in our environment.
    run(&["hostname"])
}

fn num_cpus_online() -> Result<u32> {
    let s = run(&["nproc"])?;
    s.parse()
        .with_context(|| format!("nproc output not a number: {s:?}"))
}

/// Read the kernel's reported isolated CPU set. Soft-fail to `"?"` so the
/// renderers can still emit a meaningful row when running off-host (e.g.
/// rendering an old snapshot on a developer laptop).
fn read_isolated_cpu() -> String {
    std::fs::read_to_string("/sys/devices/system/cpu/isolated")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".to_string())
}
