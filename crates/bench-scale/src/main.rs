//! How many HTTP workloads one running wasmCloud host holds with the CPU and
//! memory it has, warm and scaled to zero. See README.md.
//!
//! The bench attaches to a host it is given and talks to it only over its
//! NATS API and HTTP, so any host binary that speaks that API can be
//! measured and compared.

mod api;
mod bench;
mod cli;
mod lifecycle;
mod memory;

use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context as _, Result};
use bench_tools::density::{Flavour, print_comparison};
use clap::Parser as _;

use crate::bench::Run;
use crate::cli::{Args, FlavourArg};

/// Exit status when a flavour served fewer than --min-components.
const EXIT_TOO_FEW: u8 = 2;
/// Exit status when cleanup left workloads on the host (the error status).
const EXIT_CLEANUP_FAILED: u8 = 1;

fn main() -> ExitCode {
    let args = Args::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("bench-scale: starting the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        let cancel = tokio_util::sync::CancellationToken::new();
        let shutdown = shutdown_signal()?;
        lifecycle::until_shutdown(&cancel, shutdown, run(args, &cancel)).await
    });
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("bench-scale: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn shutdown_signal() -> std::io::Result<impl std::future::Future<Output = std::io::Result<()>>> {
    // Register SIGTERM before starting any work: job runners and containers use it.
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    Ok(async move {
        #[cfg(unix)]
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await
    })
}

async fn run(args: Args, cancel: &tokio_util::sync::CancellationToken) -> Result<u8> {
    let memory_limit = args.memory_limit_bytes()?;
    let nats = lifecycle::cancellable(cancel, async {
        async_nats::connect(&args.nats_url)
            .await
            .context("connecting to --nats-url")
    })
    .await?;
    let client = api::HostClient::new(nats, args.host_id.clone());
    let heartbeat = client
        .heartbeat(Duration::from_secs(10))
        .await
        .map_err(|e| anyhow::anyhow!("host {} did not answer a heartbeat: {e}", args.host_id))?;
    let run_id = uuid::Uuid::new_v4().simple().to_string();
    let run = Run {
        args: &args,
        memory_limit,
        run_id: &run_id,
        client: &client,
        subject: format!(
            "{} on wash {} ({})",
            args.image, heartbeat.version, heartbeat.id
        ),
        cancel,
    };

    let flavours: &[Flavour] = match args.flavour {
        FlavourArg::Warm => &[Flavour::Warm],
        FlavourArg::ScaleToZero => &[Flavour::ScaleToZero],
        FlavourArg::Both => &Flavour::ALL,
    };
    let mut reports = Vec::new();
    let mut cleanup_failed = false;
    for &flavour in flavours {
        if args.report {
            eprintln!("\n== {} ==", flavour.name());
        }
        let ((report, stop), cleanup_error) = bench::run_flavour(&run, flavour).await?;
        if let Some(diagnostic) = stop.diagnostic() {
            eprintln!("bench-scale: {}: {stop}: {diagnostic}", flavour.name());
        }
        reports.push(report);
        if let Some(e) = cleanup_error {
            // Workloads left on the host would skew the next flavour; report what ran.
            eprintln!("bench-scale: {}: {e}", flavour.name());
            cleanup_failed = true;
            break;
        }
    }

    let too_few = reports.iter().any(|r| r.served() < args.min_components);
    if args.report {
        print_comparison(&reports);
        if too_few {
            eprintln!(
                "FAIL: a flavour served fewer than {} workloads",
                args.min_components
            );
        }
    }
    if let Some(path) = &args.json {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&reports)?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(if cleanup_failed {
        EXIT_CLEANUP_FAILED
    } else if too_few {
        EXIT_TOO_FEW
    } else {
        0
    })
}
