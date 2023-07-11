#![warn(clippy::pedantic)]

use anyhow::{self, Context};
use clap::Parser;
use tokio::signal;
use tracing_subscriber::prelude::*;
use wasmcloud_host::WasmbusLatticeConfig;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Args = Args::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn")
            }),
        )
        .init();

    let (_, shutdown) = wasmcloud_host::WasmbusLattice::new(WasmbusLatticeConfig::default())
        .await
        .context("failed to initialize `wasmbus` lattice")?;
    signal::ctrl_c().await?;
    shutdown.await.context("failed to shutdown lattice")?;
    Ok(())
}
