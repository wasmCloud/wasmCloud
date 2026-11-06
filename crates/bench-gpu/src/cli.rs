//! The `run` and `turns` commands.

use std::{path::PathBuf, process::ExitCode};

use anyhow::{Context as _, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

use crate::{
    turns,
    workload::{Options, Runner, canonical_mounts},
};

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one command component once, then write a JSON report.
    Run(RunArgs),
    /// Run a plan of cold-started turns, writing NDJSON events.
    Turns(TurnsArgs),
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    host: Options,
    /// Where to write the report; stderr when unset.
    #[arg(long)]
    report: Option<PathBuf>,
    /// The command component.
    wasm: PathBuf,
    /// Its arguments, program name first.
    #[arg(last = true, required = true)]
    args: Vec<String>,
}

#[derive(Args)]
struct TurnsArgs {
    #[command(flatten)]
    host: Options,
    /// The plan: `{"turns":[{"name","wasm","native","args"}],"rounds","idle_ms"}`.
    #[arg(long)]
    plan: PathBuf,
    /// Where to write events, one JSON object per line.
    #[arg(long)]
    events: PathBuf,
    /// Run each turn's `native` binary as a process instead of its `wasm`
    /// as a workload.
    #[arg(long)]
    native: bool,
}

#[tokio::main]
pub(crate) async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match run(Cli::parse()).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(err) => {
            eprintln!("bench-gpu: {err:#}");
            ExitCode::from(2)
        }
    }
}

async fn run(cli: Cli) -> Result<bool> {
    match cli.command {
        Command::Run(args) => {
            let fuel = args.host.hostcall_fuel;
            let runner = Runner::new(args.host)?;
            let wasm = std::fs::read(&args.wasm)
                .with_context(|| format!("read {}", args.wasm.display()))?;
            let outcome = runner.run("command", wasm, &args.args).await?;
            let report = json!({
                "exit_code": outcome.exit_code,
                "error": outcome.error,
                "resolve_ms": outcome.resolve_ms,
                "run_ms": outcome.run_ms,
                "guest_memory_peak": runner.guest_memory_peak(),
                "hostcall_fuel": fuel,
            });
            match &args.report {
                Some(path) => std::fs::write(path, serde_json::to_vec_pretty(&report)?)
                    .with_context(|| format!("write {}", path.display()))?,
                None => eprintln!("{report}"),
            }
            Ok(outcome.exit_code == Some(0))
        }
        Command::Turns(args) => {
            let plan = turns::load_plan(&args.plan)?;
            let mut events = std::fs::File::create(&args.events)
                .with_context(|| format!("create {}", args.events.display()))?;
            if args.native {
                let mounts = canonical_mounts(args.host.mounts)?;
                turns::run(&turns::Side::Native(&mounts), &plan, &mut events).await
            } else {
                let runner = Runner::new(args.host)?;
                turns::run(&turns::Side::Wasm(&runner), &plan, &mut events).await
            }
        }
    }
}
