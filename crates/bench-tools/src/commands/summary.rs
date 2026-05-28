//! `bench-tools summary` renders a markdown table for `$GITHUB_STEP_SUMMARY`
//!
//! Per-row metric is RPS for batch throughput, B/s for byte throughput,
//! and time otherwise. For `gungraun` we render a separate
//! instruction-count table from the callgrind output with different
//! schema and unit, but same rough shape so the step summary stays
//! consistent.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;

use crate::callgrind;
use crate::criterion;
use crate::markdown;
use crate::meta::Meta;

#[derive(Debug, clap::Args)]
pub struct Args {
    #[arg(long)]
    pub bench: String,

    #[arg(long, env = "CARGO_TARGET_DIR")]
    pub target_dir: Option<PathBuf>,

    /// GitHub Actions run id (`$GITHUB_RUN_ID`). Used in the
    /// download-artifact pointer.
    #[arg(long, env = "GITHUB_RUN_ID")]
    pub run_id: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    let target_dir = args.target_dir.unwrap_or_else(|| PathBuf::from("target"));
    let meta = Meta::capture()?;
    let run_id = args.run_id.unwrap_or_else(|| "local".to_string());

    // Single locked handle for the whole render — fewer syscalls and no
    // interleaving if anything else writes to stdout.
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    if args.bench == "gungraun" {
        return render_gungraun_stub(&mut out, &args.bench, &meta, &target_dir, &run_id);
    }

    let crit_dir = criterion::dir_from_target(&target_dir);
    let marker = target_dir.join(format!(".bench-start-{}", args.bench));
    let rows = criterion::walk(&crit_dir, Some(&marker))?;
    if rows.is_empty() {
        writeln!(
            out,
            "## bench: `{}`\n\n_no criterion output at `{}`_",
            args.bench,
            crit_dir.display()
        )?;
        return Ok(());
    }

    render_criterion(&mut out, &args.bench, &meta, rows, &run_id)
}

fn render_criterion<W: Write>(
    out: &mut W,
    bench: &str,
    meta: &Meta,
    rows: Vec<criterion::Row>,
    run_id: &str,
) -> Result<()> {
    writeln!(out, "## bench: `{bench}`")?;
    writeln!(out)?;
    writeln!(
        out,
        "_commit_ `{}` &middot; _ref_ `{}` &middot; _host_ `{}` &middot; _cpus_ {}",
        meta.short_sha, meta.ref_name, meta.host, meta.cpus_online,
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "| group | param | metric | mean | median | std-dev | 95 % CI |"
    )?;
    writeln!(out, "|---|---|---|---:|---:|---:|---:|")?;
    for row in &rows {
        let (unit, vals) = markdown::metric(row);
        writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {}–{} |",
            row.group,
            row.param,
            unit.label(),
            markdown::fmt(vals.mean, unit),
            markdown::fmt(vals.median, unit),
            markdown::fmt(vals.std_dev, unit),
            markdown::fmt(vals.ci_low, unit),
            markdown::fmt(vals.ci_high, unit),
        )?;
    }
    writeln!(out)?;
    writeln!(out, "<details><summary>raw criterion output</summary>")?;
    writeln!(out)?;
    writeln!(
        out,
        "Download the workflow artifact `bench-{bench}-{run_id}` or fetch \
         from S3 (see scripts/bench/README.md).",
    )?;
    writeln!(out)?;
    writeln!(out, "</details>")?;
    Ok(())
}

fn render_gungraun_stub<W: Write>(
    out: &mut W,
    bench: &str,
    meta: &Meta,
    target_dir: &std::path::Path,
    run_id: &str,
) -> Result<()> {
    writeln!(out, "## bench: `{bench}` (gungraun · cachegrind)")?;
    writeln!(out)?;
    writeln!(
        out,
        "_commit_ `{}` &middot; _ref_ `{}` &middot; _host_ `{}` &middot; _cpus_ {}",
        meta.short_sha, meta.ref_name, meta.host, meta.cpus_online,
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "Instruction-count bench (cachegrind via gungraun). Raw counts \
         and callgrind output are archived under the workflow artifact \
         `bench-{bench}-{run_id}` and in S3 (see scripts/bench/README.md).",
    )?;
    writeln!(out)?;

    let gungraun_dir = callgrind::dir_from_target(target_dir);
    let rows = callgrind::walk(&gungraun_dir)?;
    if !rows.is_empty() {
        writeln!(out, "| group | param | Ir (instructions) |")?;
        writeln!(out, "|---|---|---:|")?;
        for row in rows {
            writeln!(
                out,
                "| {} | {} | {} |",
                row.group,
                row.param,
                markdown::fmt_thousands(row.ir),
            )?;
        }
    }
    Ok(())
}
