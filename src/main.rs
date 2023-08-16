#![warn(clippy::pedantic)]

use anyhow::{self, Context};
use clap::Parser;
use tokio::{select, signal};
use tracing_subscriber::prelude::*;
use wasmcloud_host::url::Url;
use wasmcloud_host::WasmbusHostConfig;

/// Default NATS server host
const DEFAULT_NATS_HOST: &str = "localhost";
/// Default NATS server port
const DEFAULT_NATS_PORT: &str = "4222";
/// Default lattice prefix
const DEFAULT_LATTICE_PREFIX: &str = "default";

/// Env var for NATS server host
const ENV_NATS_HOST: &str = "NATS_HOST";
/// Env var for NATS server port
const ENV_NATS_PORT: &str = "NATS_PORT";
/// Env var for lattice prefix
const ENV_LATTICE_PREFIX: &str = "WASMCLOUD_LATTICE_PREFIX";
/// Env var for host seed
const ENV_HOST_SEED: &str = "WASMCLOUD_HOST_SEED";
/// Env var for cluster seed
const ENV_CLUSTER_SEED: &str = "WASMCLOUD_CLUSTER_SEED";

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// NATS server host to connect to
    #[clap(long = "nats-host", default_value = DEFAULT_NATS_HOST, env = ENV_NATS_HOST)]
    pub nats_host: String,
    /// NATS server port to connect to
    #[clap(long = "nats-port", default_value = DEFAULT_NATS_PORT, env = ENV_NATS_PORT)]
    pub nats_port: u16,
    /// The lattice the host belongs to
    #[clap(short = 'x', long = "lattice-prefix", default_value = DEFAULT_LATTICE_PREFIX, env = ENV_LATTICE_PREFIX)]
    pub lattice_prefix: String,
    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to generate its public key  
    #[clap(long = "host-seed", env = ENV_HOST_SEED)]
    pub host_seed: Option<String>,
    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to sign all invocations
    #[clap(long = "cluster-seed", env = ENV_CLUSTER_SEED)]
    pub cluster_seed: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Args {
        nats_host,
        nats_port,
        lattice_prefix,
        host_seed,
        cluster_seed,
    } = Args::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn")
            }),
        )
        .init();

    let ctl_nats_url = Url::parse(&format!("nats://{nats_host}:{nats_port}"))
        .context("failed to construct a valid `ctl_nats_url` using `nats-host` and `nats-port`")?;
    let (host, shutdown) = wasmcloud_host::wasmbus::Host::new(WasmbusHostConfig {
        ctl_nats_url,
        lattice_prefix,
        host_seed,
        cluster_seed,
    })
    .await
    .context("failed to initialize host")?;
    select! {
        sig = signal::ctrl_c() => sig.context("failed to wait for Ctrl-C")?,
        _ = host.stopped() => {},
    };
    shutdown.await.context("failed to shutdown host")?;
    Ok(())
}
