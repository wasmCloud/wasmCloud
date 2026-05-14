//! Internal CLI for processing wasmCloud bench output (criterion + iai-callgrind).
//!
//! Run via `cargo run -p bench-tools -- <subcommand>` from CI or from any
//! script in `scripts/bench/`. The output schemas match what the trend
//! site (`wasmCloud/arewefastyet`) ingests from `history.json`.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod callgrind;
mod commands;
mod criterion;
mod markdown;
mod meta;

#[derive(Debug, Parser)]
#[command(
    name = "bench-tools",
    version,
    about = "wasmCloud bench data processing (criterion + iai-callgrind)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Emit one JSONL row per (group, param) for trend ingestion.
    Jsonl(commands::jsonl::Args),

    /// Render a markdown table for $GITHUB_STEP_SUMMARY.
    Summary(commands::summary::Args),

    /// Render a markdown comparison delta from compare-bench.sh snapshots.
    Delta(commands::delta::Args),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Jsonl(args) => commands::jsonl::run(args),
        Cmd::Summary(args) => commands::summary::run(args),
        Cmd::Delta(args) => commands::delta::run(args),
    }
}
