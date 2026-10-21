//! Command-line settings. Every flag falls back to a `BENCH_SCALE_*`
//! variable, so CI can configure a run without editing its command.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, ValueEnum};
use wash_runtime::engine::host_memory::{detected_memory_limit, parse_bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FlavourArg {
    Warm,
    ScaleToZero,
    Both,
}

/// How many HTTP workloads one running host holds, warm and scaled to zero,
/// talking to it only over its NATS API and HTTP.
#[derive(Debug, Clone, Parser)]
#[command(name = "bench-scale", version)]
pub struct Args {
    /// NATS server the host is connected to.
    // The URL can carry credentials, so `--help` names the variable only.
    #[arg(long, env = "BENCH_SCALE_NATS_URL", hide_env_values = true)]
    pub nats_url: String,

    /// The host to bench, by the id it heartbeats with.
    #[arg(long, env = "BENCH_SCALE_HOST_ID")]
    pub host_id: String,

    /// The host's HTTP listener (`addr:port`); requests are routed by Host header.
    #[arg(long, env = "BENCH_SCALE_HOST_HTTP")]
    pub host_http: String,

    /// The host's process id, for memory readings. Without it nothing
    /// bounds the run but --max-components.
    #[arg(long, env = "BENCH_SCALE_HOST_PID")]
    pub host_pid: Option<u32>,

    /// The P3 HTTP component image the host pulls for every workload. Warm
    /// runs need one exporting `wasi:http/handler@0.3.0`; a P2 instance
    /// cannot stay warm, and its failed warm-ups stop the run.
    #[arg(long, env = "BENCH_SCALE_IMAGE")]
    pub image: String,

    /// Which flavour to run; `both` runs warm, then scale-to-zero.
    #[arg(long, value_enum, default_value = "both", env = "BENCH_SCALE_FLAVOUR")]
    pub flavour: FlavourArg,

    /// The footprint at which the bench stops (e.g. 4GiB). Default: half
    /// this machine's memory limit (the smaller of RAM and the cgroup's).
    #[arg(long, env = "BENCH_SCALE_MEMORY_LIMIT")]
    pub memory_limit: Option<String>,

    /// Stop after this many workloads. Default: the theoretical limit
    /// measured so far times --overshoot.
    #[arg(long, env = "BENCH_SCALE_MAX_COMPONENTS")]
    pub max_components: Option<usize>,

    /// How far past the theoretical limit the default cap goes.
    #[arg(long, default_value_t = 1.1, env = "BENCH_SCALE_OVERSHOOT")]
    pub overshoot: f64,

    /// Workloads started between measurements.
    #[arg(long, default_value_t = 250, env = "BENCH_SCALE_STEP")]
    pub step: usize,

    /// Start requests in flight at once. The host answers at most 64 pending
    /// starts and drops the rest unanswered.
    #[arg(long, default_value_t = 32, env = "BENCH_SCALE_START_CONCURRENCY")]
    pub start_concurrency: usize,

    /// Seconds to wait for a start's reply before asking its status.
    #[arg(long, default_value_t = 60, env = "BENCH_SCALE_START_TIMEOUT")]
    pub start_timeout: u64,

    /// Times one start is retried after no reply or no responders.
    #[arg(long, default_value_t = 3, env = "BENCH_SCALE_START_RETRIES")]
    pub start_retries: u32,

    /// Requests per step, spread evenly across every started workload.
    #[arg(long, default_value_t = 50, env = "BENCH_SCALE_SAMPLE")]
    pub sample: usize,

    /// HTTP requests in flight at once.
    #[arg(long, default_value_t = 64, env = "BENCH_SCALE_REQUEST_CONCURRENCY")]
    pub request_concurrency: usize,

    /// Seconds before an HTTP request counts as failed.
    #[arg(long, default_value_t = 30, env = "BENCH_SCALE_REQUEST_TIMEOUT")]
    pub request_timeout: u64,

    /// Scale-to-zero: milliseconds to wait for the footprint to settle back
    /// after a sample before reporting no spin-down.
    #[arg(long, default_value_t = 3000, env = "BENCH_SCALE_SPIN_DOWN_WAIT")]
    pub spin_down_wait: u64,

    /// Exit with status 2 if a flavour served fewer workloads than this.
    #[arg(long, default_value_t = 1, env = "BENCH_SCALE_MIN_COMPONENTS")]
    pub min_components: usize,

    /// Print each step and the comparison on stderr (otherwise silent).
    #[arg(long, env = "BENCH_SCALE_REPORT")]
    pub report: bool,

    /// Write the results here as a list of `bench_tools::density::Report`,
    /// which `bench-tools jsonl` publishes when it is
    /// `<target>/density/bench-scale.json`.
    #[arg(long, env = "BENCH_SCALE_JSON")]
    pub json: Option<PathBuf>,
}

impl Args {
    /// Reject settings that cannot measure anything, before touching the
    /// host, and resolve the memory limit.
    pub fn memory_limit_bytes(&self) -> Result<u64> {
        bench_tools::density::sample_size(self.sample).map_err(anyhow::Error::msg)?;
        if self.step == 0 || self.start_concurrency == 0 || self.request_concurrency == 0 {
            bail!("--step, --start-concurrency and --request-concurrency must be at least 1");
        }
        if self.overshoot.is_nan() || self.overshoot < 1.0 {
            bail!("--overshoot must be at least 1.0");
        }
        if self.host_pid.is_none() && self.max_components.is_none() {
            bail!(
                "without --host-pid the bench cannot see memory, so nothing bounds the run; \
                 pass --host-pid or --max-components"
            );
        }
        match &self.memory_limit {
            Some(limit) => parse_bytes(limit)
                .map_err(anyhow::Error::msg)
                .context("--memory-limit"),
            None => Ok(detected_memory_limit()
                .context("cannot detect a memory limit; pass --memory-limit")?
                / 2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    const REQUIRED: [&str; 9] = [
        "bench-scale",
        "--nats-url",
        "nats://127.0.0.1:4222",
        "--host-id",
        "NABC",
        "--host-http",
        "127.0.0.1:8080",
        "--image",
        "localhost:5000/http:v1",
    ];

    fn args(extra: &[&str]) -> Args {
        Args::try_parse_from(REQUIRED.iter().chain(extra)).unwrap()
    }

    /// `--help` prints each variable's current value unless told not to, and
    /// a NATS URL can carry a user and password.
    #[test]
    fn help_never_prints_the_nats_url() {
        let command = Args::command();
        let nats_url = command
            .get_arguments()
            .find(|arg| arg.get_id() == "nats_url")
            .expect("the --nats-url argument");
        assert!(nats_url.is_hide_env_values_set());
    }

    #[test]
    fn unmeasurable_runs_are_rejected_before_touching_the_host() {
        assert!(
            args(&["--host-pid", "1", "--sample", "0"])
                .memory_limit_bytes()
                .is_err()
        );
        // Without a PID only a cap bounds the run.
        assert!(
            args(&["--memory-limit", "1GiB"])
                .memory_limit_bytes()
                .is_err()
        );
        assert_eq!(
            args(&["--max-components", "10", "--memory-limit", "1GiB"])
                .memory_limit_bytes()
                .unwrap(),
            1 << 30
        );
    }
}
