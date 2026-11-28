//! Build once, then run each pressure case in its own bounded cgroup.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, ensure};
use bench_tools::pressure::{
    CASE_ENV, Case, DIRTY_MIB, Exit, MEMORY_MIB, Mode, Outcome, REPORT_ENV, Report, Shape,
    classify, print_summary,
};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Cow {
    On,
    Off,
}

#[derive(Debug, clap::Args)]
pub struct Args {
    #[arg(long, num_args = 1.., default_values = ["384", "512"], value_parser = clap::value_parser!(u64).range(MEMORY_MIB))]
    memory_mib: Vec<u64>,
    #[arg(long, num_args = 1.., value_enum, default_values = ["sparse", "data-heavy", "multiple", "dirty"])]
    shapes: Vec<Shape>,
    #[arg(long, num_args = 1.., value_enum, default_values = ["enforce", "growth"])]
    modes: Vec<Mode>,
    #[arg(long, num_args = 1.., value_enum, default_values = ["on", "off"])]
    cow: Vec<Cow>,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(DIRTY_MIB))]
    dirty_mib: u64,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    initial_cap_mib: Option<u64>,
    /// Require sparse growth coverage with unmodified runtime memory budgets.
    #[arg(long, conflicts_with_all = ["initial_cap_mib", "guest_memory_mib"])]
    require_runtime_defaults: bool,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    guest_memory_mib: Option<u64>,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    repeats: u32,
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..))]
    timeout: u64,
    /// Timeout for each Docker control command, in seconds.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
    docker_timeout: u64,
    #[arg(long, default_value = "wash-pressure-target")]
    target_volume: String,
    #[arg(long, default_value = "wash-pressure-registry")]
    registry_volume: String,
    /// Shared benchmark target directory; results go under pressure/.
    #[arg(long, env = "CARGO_TARGET_DIR", default_value = "target")]
    target_dir: PathBuf,
}

/// Set once by SIGINT or SIGTERM; every later check sees it.
#[derive(Clone)]
struct Cancel(Arc<AtomicBool>);

impl Cancel {
    fn check(&self) -> Result<()> {
        ensure!(!self.0.load(Ordering::Relaxed), "pressure run interrupted");
        Ok(())
    }
}

struct Commands {
    directory: PathBuf,
    cancel: Cancel,
    timeout: Duration,
}

impl Commands {
    fn new(directory: &Path, cancel: &Cancel, timeout: Duration) -> Result<Self> {
        let directory = directory.join("commands");
        fs::create_dir(&directory)?;
        Ok(Self {
            directory,
            cancel: cancel.clone(),
            timeout,
        })
    }

    fn output(&self, name: &str, command: &mut Command) -> Result<String> {
        Ok(String::from_utf8(self.bytes(name, command)?)?
            .trim()
            .to_owned())
    }

    fn bytes(&self, name: &str, command: &mut Command) -> Result<Vec<u8>> {
        self.cancel.check()?;
        let stdout = self.directory.join(format!("{name}.stdout.log"));
        let stderr = self.directory.join(format!("{name}.stderr.log"));
        let mut process = Process(
            command
                .stdin(Stdio::null())
                .stdout(File::create(&stdout)?)
                .stderr(File::create(&stderr)?)
                .spawn()?,
        );
        let Completion::Exited(status) = process.wait(Some(&self.cancel), Some(self.timeout))?
        else {
            anyhow::bail!("{name} timed out");
        };
        ensure!(
            status.success(),
            "{name} failed: {}",
            String::from_utf8_lossy(&fs::read(stderr)?)
        );
        Ok(fs::read(stdout)?)
    }

    fn docker(&self, name: &str, args: &[&str]) -> Result<String> {
        self.output(name, Command::new("docker").args(args))
    }
}

fn build(
    args: &Args,
    root: &Path,
    artifacts: &Path,
    commands: &Commands,
) -> Result<(String, String)> {
    let toolchain = fs::read_to_string(root.join("rust-toolchain.toml"))?;
    let version = toolchain
        .lines()
        .find_map(|line| line.trim().strip_prefix("channel = \"")?.strip_suffix('"'))
        .context("missing pinned Rust channel")?;
    let image = format!("rust:{version}-bookworm");
    eprintln!("building memory_pressure in {image}; the first build can take several minutes");
    let container = Container::new(
        format!(
            "wash-pressure-build-{}-{}",
            std::process::id(),
            artifacts
                .file_name()
                .context("missing artifact name")?
                .to_string_lossy()
        ),
        commands.timeout,
    );
    let mut process = Process(
        Command::new("docker")
            .args([
                "run",
                "--name",
                &container.name,
                "--label=wasmcloud.bench=memory_pressure",
                "--memory=8g",
                "--memory-swap=8g",
                "--cpus=4",
                "--mount",
                &format!("type=bind,src={},dst=/work,readonly", root.display()),
                "--mount",
                &format!("type=volume,src={},dst=/target", args.target_volume),
                "--mount",
                &format!(
                    "type=volume,src={},dst=/usr/local/cargo/registry",
                    args.registry_volume
                ),
                "--workdir=/work",
                "--env=CARGO_TARGET_DIR=/target",
                &format!("--env=RUSTUP_TOOLCHAIN={version}"),
                &image,
                "cargo",
                "bench",
                "--no-run",
                "--locked",
                "--no-default-features",
                "-p",
                "wash-runtime",
                "--bench",
                "memory_pressure",
                "--message-format=json-render-diagnostics",
            ])
            .stdout(File::create(artifacts.join("build.jsonl"))?)
            .spawn()?,
    );
    let Completion::Exited(status) = process.wait(Some(&commands.cancel), None)? else {
        anyhow::bail!("build timed out");
    };
    ensure!(status.success(), "pressure benchmark build failed");
    let mut binary = None;
    for line in fs::read_to_string(artifacts.join("build.jsonl"))?.lines() {
        let message: Value = serde_json::from_str(line)?;
        if message.pointer("/target/name").and_then(Value::as_str) == Some("memory_pressure")
            && let Some(path) = message.get("executable").and_then(Value::as_str)
        {
            binary = Some(path.to_owned());
        }
    }
    let binary = binary.context("cargo did not report the benchmark executable")?;
    ensure!(
        binary.starts_with("/target/"),
        "executable is outside target volume"
    );
    Ok((
        commands.docker(
            "image-inspect",
            &["image", "inspect", &image, "--format={{.Id}}"],
        )?,
        binary,
    ))
}

/// Removes the named container when dropped, even after cancellation.
struct Container {
    name: String,
    timeout: Duration,
}

impl Container {
    fn new(name: String, timeout: Duration) -> Self {
        Self {
            name,
            timeout: timeout.min(Duration::from_secs(5)),
        }
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        let result = (|| -> Result<()> {
            let mut process = Process(
                Command::new("docker")
                    .args(["rm", "--force", &self.name])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()?,
            );
            let Completion::Exited(status) = process.wait(None, Some(self.timeout))? else {
                anyhow::bail!("cleanup timed out");
            };
            ensure!(status.success(), "docker rm failed with {status}");
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!(
                "warning: could not remove pressure container {}: {error}",
                self.name
            );
        }
    }
}

enum Completion {
    Exited(ExitStatus),
    TimedOut,
}

/// Kills and reaps the child when dropped.
struct Process(Child);

impl Process {
    fn wait(&mut self, cancel: Option<&Cancel>, timeout: Option<Duration>) -> Result<Completion> {
        let started = Instant::now();
        loop {
            if let Some(cancel) = cancel {
                cancel.check()?;
            }
            if let Some(status) = self.0.try_wait()? {
                return Ok(Completion::Exited(status));
            }
            if timeout.is_some_and(|timeout| started.elapsed() > timeout) {
                return Ok(Completion::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The last lines of a log, for a failure message.
fn tail(path: &Path, lines: usize) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all.get(all.len().saturating_sub(lines)..)
        .unwrap_or_default()
        .join(" | ")
}

fn run_case(
    args: &Args,
    artifacts: &Path,
    image: &str,
    binary: &str,
    case: &Case,
    cancel: &Cancel,
) -> Result<Report> {
    let label = case.label();
    let directory = artifacts.join(&label);
    fs::create_dir(&directory)?;
    let commands = Commands::new(&directory, cancel, Duration::from_secs(args.docker_timeout))?;
    let name = format!(
        "wash-pressure-{}-{}-{label}",
        std::process::id(),
        artifacts
            .file_name()
            .context("missing artifact name")?
            .to_string_lossy()
    );
    let container = Container::new(name.clone(), commands.timeout);
    let mut create = Command::new("docker");
    create.args([
        "create",
        "--name",
        &name,
        "--network=none",
        "--label=wasmcloud.bench=memory_pressure",
        "--cpus=2",
        &format!("--memory={}m", case.memory_mib),
        &format!("--memory-swap={}m", case.memory_mib),
        "--mount",
        &format!(
            "type=volume,src={},dst=/target,readonly",
            args.target_volume
        ),
        "--mount",
        &format!("type=bind,src={},dst=/results", directory.display()),
        &format!("--env={REPORT_ENV}=/results/report.json"),
        &format!("--env={CASE_ENV}={}", serde_json::to_string(case)?),
        image,
        binary,
        "--bench",
    ]);
    commands.output("create", &mut create)?;
    let mut attached = Process(
        Command::new("docker")
            .args(["start", "--attach", &container.name])
            .stdout(File::create(directory.join("stdout.log"))?)
            .stderr(File::create(directory.join("stderr.log"))?)
            .spawn()?,
    );
    let timed_out = matches!(
        attached.wait(Some(cancel), Some(Duration::from_secs(args.timeout)))?,
        Completion::TimedOut
    );
    if timed_out {
        // The container may exit between the deadline and the kill.
        if let Err(error) = commands.docker("kill", &["kill", &container.name]) {
            cancel.check()?;
            eprintln!("warning: docker kill {}: {error:#}", container.name);
        }
        ensure!(
            matches!(
                attached.wait(Some(cancel), Some(commands.timeout))?,
                Completion::Exited(_)
            ),
            "Docker attachment did not exit after kill"
        );
    }
    let inspected: Value =
        serde_json::from_str(&commands.docker("inspect", &["inspect", &container.name])?)?;
    fs::write(
        directory.join("docker.json"),
        serde_json::to_vec_pretty(&inspected)?,
    )?;
    let exit: Exit = serde_json::from_value(
        inspected
            .pointer("/0/State")
            .context("missing Docker state")?
            .clone(),
    )?;
    let mut report = match fs::read(directory.join("report.json")) {
        Ok(bytes) => serde_json::from_slice::<Report>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Report::new(case.clone()),
        Err(error) => return Err(error.into()),
    };
    ensure!(&report.case == case, "report describes a different case");
    let result = if timed_out {
        Err("timed out".into())
    } else {
        classify(&report, &exit)
    };
    let outcome = result.unwrap_or_else(|error| {
        let stderr = tail(&directory.join("stderr.log"), 3);
        report.stop = format!("{error}; admission: {}; stderr: {stderr}", report.stop);
        Outcome::Failed
    });
    report.outcome = Some(outcome);
    eprintln!(
        "{} {label}: {}; admitted={}",
        if outcome == Outcome::Failed {
            "FAIL"
        } else {
            "PASS"
        },
        outcome.name(),
        report.admission_count()
    );
    fs::write(
        directory.join("validated.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}

/// Drop repeated values, keeping first-seen order; a repeat would reuse a case's directory.
fn dedup<T: PartialEq>(values: &mut Vec<T>) {
    let mut kept: Vec<T> = Vec::with_capacity(values.len());
    for value in values.drain(..) {
        if !kept.contains(&value) {
            kept.push(value);
        }
    }
    *values = kept;
}

pub fn run(mut args: Args) -> Result<()> {
    dedup(&mut args.memory_mib);
    dedup(&mut args.shapes);
    dedup(&mut args.modes);
    dedup(&mut args.cow);
    ensure!(
        !args.require_runtime_defaults
            || (args.modes.contains(&Mode::Growth) && args.shapes.contains(&Shape::Sparse)),
        "--require-runtime-defaults requires sparse growth cases"
    );
    let cancel = Cancel(Arc::new(AtomicBool::new(false)));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let shutdown = {
        let _entered = runtime.enter();
        termination()?
    };
    {
        let cancel = Arc::clone(&cancel.0);
        std::thread::spawn(move || {
            runtime.block_on(shutdown);
            cancel.store(true, Ordering::Relaxed);
        });
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    fs::create_dir_all(&args.target_dir)?;
    let output = args.target_dir.canonicalize()?.join("pressure");
    fs::create_dir_all(&output)?;
    let aggregate = output.join("memory_pressure.json");
    match fs::remove_file(&aggregate) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let artifacts = output.join(format!("memory_pressure-{stamp}"));
    fs::create_dir(&artifacts)?;
    eprintln!("pressure artifacts: {}", artifacts.display());
    let commands = Commands::new(
        &artifacts,
        &cancel,
        Duration::from_secs(args.docker_timeout),
    )?;
    let (image, binary) = build(&args, &root, &artifacts, &commands)?;
    fs::write(
        artifacts.join("source.patch"),
        commands.bytes(
            "git-diff",
            Command::new("git")
                .args(["diff", "--binary", "HEAD"])
                .current_dir(&root),
        )?,
    )?;
    let git = |name, args: &[&str]| {
        commands.output(name, Command::new("git").args(args).current_dir(&root))
    };
    fs::write(
        artifacts.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "revision": git("git-revision", &["rev-parse", "HEAD"])?,
            "dirty": !git("git-status", &["status", "--porcelain"])?.is_empty(),
            "runtime_defaults_required": args.require_runtime_defaults,
            "image": image, "binary": binary,
            "build": {"cpus": 4, "memory_mib": 8192}, "measured": {"cpus": 2, "swap_mib": 0},
        }))?,
    )?;
    let mut reports = Vec::new();
    // Repeats run outermost, so a drifting host spreads across every case.
    for repeat in 1..=args.repeats {
        for &memory_mib in &args.memory_mib {
            for &shape in &args.shapes {
                for &cow in &args.cow {
                    for &mode in &args.modes {
                        let case = Case {
                            memory_mib,
                            shape,
                            mode,
                            cow: cow == Cow::On,
                            dirty_mib: args.dirty_mib,
                            guest_memory_mib: args.guest_memory_mib,
                            initial_cap_mib: args.initial_cap_mib,
                            repeat,
                        };
                        cancel.check()?;
                        // A runner error fails this case only; cancellation stops the run.
                        let report =
                            match run_case(&args, &artifacts, &image, &binary, &case, &cancel) {
                                Ok(report) => report,
                                Err(error) => {
                                    cancel.check()?;
                                    eprintln!("FAIL {}: runner error: {error:#}", case.label());
                                    let mut report = Report::new(case);
                                    report.stop = format!("runner error: {error:#}");
                                    report.outcome = Some(Outcome::Failed);
                                    report
                                }
                            };
                        reports.push(report);
                        fs::write(&aggregate, serde_json::to_vec_pretty(&reports)?)?;
                    }
                }
            }
        }
    }
    cancel.check()?;
    print_summary(&reports);
    let failed = reports
        .iter()
        .filter(|r| r.outcome == Some(Outcome::Failed))
        .count();
    eprintln!(
        "{}/{} pressure cases passed",
        reports.len() - failed,
        reports.len()
    );
    ensure!(
        failed == 0,
        "{failed} pressure cases failed; see {}",
        artifacts.display()
    );
    Ok(())
}

fn termination() -> std::io::Result<impl std::future::Future<Output = ()>> {
    #[cfg(unix)]
    {
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        Ok(async move {
            tokio::select! {
                _ = interrupt.recv() => (),
                _ = terminate.recv() => (),
            }
        })
    }
    #[cfg(windows)]
    {
        let mut interrupt = tokio::signal::windows::ctrl_c()?;
        Ok(async move {
            interrupt.recv().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn defaults_cover_small_container_regressions() {
        let args = Cli::parse_from(["pressure", "--require-runtime-defaults"]).args;
        assert_eq!(args.memory_mib, [384, 512]);
        assert_eq!(args.modes, [Mode::Enforce, Mode::Growth]);
        assert!(args.shapes.contains(&Shape::Sparse));
        assert!(args.shapes.contains(&Shape::Dirty));
        assert_eq!(args.cow, [Cow::On, Cow::Off]);
    }

    #[test]
    fn runtime_default_checks_reject_budget_overrides() {
        for flag in ["--initial-cap-mib", "--guest-memory-mib"] {
            assert!(
                Cli::try_parse_from(["pressure", "--require-runtime-defaults", flag, "256"])
                    .is_err()
            );
        }
    }

    #[test]
    fn runtime_default_checks_require_sparse_growth() {
        for flags in [["--shapes", "dirty"], ["--modes", "enforce"]] {
            let args =
                Cli::parse_from(["pressure", "--require-runtime-defaults", flags[0], flags[1]])
                    .args;
            assert!(
                run(args)
                    .unwrap_err()
                    .to_string()
                    .contains("requires sparse growth")
            );
        }
    }
}
