//! Command-line settings. Every flag falls back to a `BENCH_SCALE_*`
//! variable, so CI can configure a run without editing its command.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FlavourArg {
    Warm,
    ScaleToZero,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HostCli {
    /// `<bin> host …` with the wash flags the binary's `host --help` lists.
    Wash,
    /// Only the `--host-arg`s, with `{nats_url}`, `{memory_limit}`,
    /// `{guest_memory}`, `{core_instances}` and `{http_addr}` substituted.
    Raw,
}

/// How many HTTP workloads one host holds, warm and scaled to zero, talking to
/// it only over its NATS API and HTTP.
#[derive(Debug, Clone, Parser)]
#[command(name = "bench-scale", version)]
pub struct Args {
    /// Which flavour to run; `both` runs each against a fresh host.
    #[arg(long, value_enum, default_value = "both", env = "BENCH_SCALE_FLAVOUR")]
    pub flavour: FlavourArg,

    /// The host's memory budget (e.g. 4GiB): passed to the host where it
    /// takes one, and the footprint at which the bench stops. Default: half
    /// of the smaller of physical memory and the cgroup limit, leaving room
    /// for the benchmark, registry, and NATS processes.
    #[arg(long, env = "BENCH_SCALE_MEMORY_LIMIT")]
    pub memory_limit: Option<String>,

    /// Stop after this many workloads. Default: the theoretical limit
    /// measured so far times --overshoot.
    #[arg(long, env = "BENCH_SCALE_MAX_COMPONENTS")]
    pub max_components: Option<usize>,

    /// How far past the theoretical limit the default cap goes.
    #[arg(long, default_value_t = 1.1, env = "BENCH_SCALE_OVERSHOOT")]
    pub overshoot: f64,

    /// Workloads started between measurements. Also bounds how far one step
    /// can overshoot the memory limit.
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

    /// Pool slots passed to the host. Default: what a large host would
    /// derive: min(usable address space / 2 / (4 GiB + 32 MiB),
    /// vm.max_map_count / 5).
    #[arg(long, env = "BENCH_SCALE_CORE_INSTANCES")]
    pub core_instances: Option<u32>,

    /// Core instances one warm workload holds, for the pool's share of the
    /// theoretical limit. Omit to leave the pool's contribution unknown.
    #[arg(long, env = "BENCH_SCALE_CORE_INSTANCES_PER_WORKLOAD")]
    pub core_instances_per_workload: Option<u32>,

    /// Exit with status 2 if a flavour served fewer workloads than this.
    #[arg(long, default_value_t = 1, env = "BENCH_SCALE_MIN_COMPONENTS")]
    pub min_components: usize,

    /// Print the human report (otherwise the run is silent).
    #[arg(long, env = "BENCH_SCALE_REPORT")]
    pub report: bool,

    /// Write the full results as JSON here, report or not.
    #[arg(long, env = "BENCH_SCALE_JSON")]
    pub json: Option<PathBuf>,

    /// Where host and NATS logs go. Default: <cache-dir>/runs/<unix time>.
    #[arg(long, env = "BENCH_SCALE_RUN_DIR")]
    pub run_dir: Option<PathBuf>,

    /// Downloads and other reusable state. Default: target/bench-scale in
    /// the workspace.
    #[arg(long, env = "BENCH_SCALE_CACHE_DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Host binary to run. Default: the released wash for --wash-version.
    #[arg(long, env = "BENCH_SCALE_HOST_BIN")]
    pub host_bin: Option<PathBuf>,

    /// How to build the host's command line.
    #[arg(long, value_enum, default_value = "wash", env = "BENCH_SCALE_HOST_CLI")]
    pub host_cli: HostCli,

    /// Extra argument for the host command (repeatable).
    #[arg(long = "host-arg", allow_hyphen_values = true, value_name = "ARG")]
    pub host_args: Vec<String>,

    /// RUST_LOG for a spawned host.
    #[arg(long, default_value = "warn", env = "BENCH_SCALE_HOST_LOG")]
    pub host_log: String,

    /// Released wash to download when no --host-bin is given.
    /// Default: this workspace's version.
    #[arg(long, env = "BENCH_SCALE_WASH_VERSION")]
    pub wash_version: Option<String>,

    /// Spawn no host: bench one already connected to --nats-url.
    #[arg(long, alias = "attach", env = "BENCH_SCALE_HOST_NATS_ONLY")]
    pub host_nats_only: bool,

    /// Which existing host to use with --host-nats-only.
    #[arg(long, env = "BENCH_SCALE_HOST_ID")]
    pub host_id: Option<String>,

    /// The attached host's process id, for memory sampling.
    #[arg(long, env = "BENCH_SCALE_HOST_PID")]
    pub host_pid: Option<u32>,

    /// Where to send HTTP requests, overriding 127.0.0.1:<heartbeat httpPort>.
    #[arg(long, env = "BENCH_SCALE_HOST_HTTP")]
    pub host_http: Option<String>,

    /// Seconds to wait for the host's first heartbeat.
    #[arg(long, default_value_t = 60, env = "BENCH_SCALE_HEARTBEAT_TIMEOUT")]
    pub heartbeat_timeout: u64,

    /// Use this NATS server instead of downloading and running one.
    // The URL can carry credentials, so `--help` names the variable only.
    #[arg(long, env = "BENCH_SCALE_NATS_URL", hide_env_values = true)]
    pub nats_url: Option<String>,

    /// nats-server release to download.
    #[arg(long, default_value = "2.12.8", env = "BENCH_SCALE_NATS_VERSION")]
    pub nats_version: String,

    /// Component image to start instead of serving the built fixture.
    /// Only scale-to-zero accepts an uninspected image; warm runs use --wasm
    /// so the benchmark can verify its poolable P3 HTTP export.
    #[arg(long, env = "BENCH_SCALE_IMAGE")]
    pub image: Option<String>,

    /// Serve this P3 HTTP component instead of building the http-handler-p3 fixture.
    #[arg(long, env = "BENCH_SCALE_WASM", conflicts_with = "image")]
    pub wasm: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

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
}
