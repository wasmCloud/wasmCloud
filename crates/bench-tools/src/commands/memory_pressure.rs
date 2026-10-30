//! Build once, then run each pressure case in its own bounded cgroup.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, ensure};
use bench_tools::density::pressure::{Case, Exit, Outcome, Shape, classify};
use bench_tools::density::{Report, print_comparison};
use serde_json::{Value, json};

#[derive(Debug, clap::Args)]
pub struct Args {
    #[arg(long, num_args = 1.., default_values = ["768"], value_parser = clap::value_parser!(u64).range(384..=2048))]
    memory_mib: Vec<u64>,
    #[arg(long, num_args = 1.., value_enum, default_values = ["sparse", "data-heavy", "multiple", "dirty"])]
    shapes: Vec<Shape>,
    #[arg(long, num_args = 1.., value_parser = ["enforce", "growth"], default_values = ["enforce", "growth"])]
    modes: Vec<String>,
    #[arg(long, num_args = 1.., value_parser = ["on", "off"], default_values = ["on", "off"])]
    cow: Vec<String>,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=2))]
    dirty_mib: u64,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    initial_cap_mib: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    guest_memory_mib: Option<u64>,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    repeats: u32,
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..))]
    timeout: u64,
    #[arg(long, default_value = "wash-pressure-target")]
    target_volume: String,
    #[arg(long, default_value = "wash-pressure-registry")]
    registry_volume: String,
    /// Shared benchmark target directory; results go under density/.
    #[arg(long, env = "CARGO_TARGET_DIR", default_value = "target")]
    target_dir: PathBuf,
}

fn output(command: &mut Command) -> Result<String> {
    let result = command.output()?;
    ensure!(
        result.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

fn docker(args: &[&str]) -> Result<String> {
    output(Command::new("docker").args(args))
}

fn build(args: &Args, root: &Path, artifacts: &Path) -> Result<(String, String)> {
    let toolchain = fs::read_to_string(root.join("rust-toolchain.toml"))?;
    let version = toolchain
        .lines()
        .find_map(|line| line.trim().strip_prefix("channel = \"")?.strip_suffix('"'))
        .context("missing pinned Rust channel")?;
    let image = format!("rust:{version}-bookworm");
    eprintln!("building memory_pressure in {image}; the first build can take several minutes");
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
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
        .status()?;
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
        docker(&["image", "inspect", &image, "--format={{.Id}}"])?,
        binary,
    ))
}

struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.0])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn run_case(
    args: &Args,
    artifacts: &Path,
    image: &str,
    binary: &str,
    case: &Case,
    interrupted: &Receiver<()>,
) -> Result<(Report, bool)> {
    let label = case.label();
    let directory = artifacts.join(&label);
    fs::create_dir(&directory)?;
    let name = format!("wash-pressure-{}-{label}", std::process::id());
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
        "--env=WASH_PRESSURE_RUN=1",
        "--env=WASH_PRESSURE_REPORT=/results/report.json",
        &format!("--env=WASH_PRESSURE_MODE={}", case.mode),
        &format!("--env=WASH_PRESSURE_COW={}", case.cow),
        &format!("--env=WASH_PRESSURE_SHAPE={}", case.shape.name()),
        &format!("--env=WASH_PRESSURE_INITIAL={}MiB", case.dirty_mib),
        &format!("--env=WASH_PRESSURE_LABEL={label}"),
    ]);
    if let Some(cap) = case.initial_cap_mib {
        create.arg(format!("--env=WASH_PRESSURE_INITIAL_CAP={cap}MiB"));
    }
    if let Some(cap) = case.guest_memory_mib {
        create.arg(format!("--env=WASH_PRESSURE_GUEST_CAP={cap}MiB"));
    }
    create.args([image, binary, "--bench"]);
    let container = Container(output(&mut create)?);
    let mut attached = Command::new("docker")
        .args(["start", "--attach", &container.0])
        .stdout(File::create(directory.join("stdout.log"))?)
        .stderr(File::create(directory.join("stderr.log"))?)
        .spawn()?;
    let started = Instant::now();
    let mut timed_out = false;
    loop {
        ensure!(interrupted.try_recv().is_err(), "pressure run interrupted");
        if attached.try_wait()?.is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(args.timeout) {
            timed_out = true;
            docker(&["kill", &container.0])?;
            attached.wait()?;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let inspected: Value = serde_json::from_str(&docker(&["inspect", &container.0])?)?;
    let state = inspected
        .pointer("/0/State")
        .context("missing Docker state")?;
    fs::write(
        directory.join("docker.json"),
        serde_json::to_vec_pretty(&inspected)?,
    )?;
    fs::write(
        directory.join("case.json"),
        serde_json::to_vec_pretty(case)?,
    )?;
    let exit: Exit = serde_json::from_value(state.clone())?;
    let mut report = if directory.join("report.json").exists() {
        let reports: Vec<Report> =
            serde_json::from_slice(&fs::read(directory.join("report.json"))?)?;
        reports
            .into_iter()
            .next()
            .context("empty pressure report")?
    } else {
        Report {
            bench: "memory_pressure".into(),
            label: label.clone(),
            stop: "no report before exit".into(),
            ..Default::default()
        }
    };
    let result = if timed_out {
        Err("timed out".into())
    } else {
        classify(case, &exit, &report)
    };
    let passed = result.is_ok();
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            report.stop = format!("{error}; admission: {}", report.stop);
            for step in &mut report.steps {
                step.clean = false;
            }
            Outcome::Failed
        }
    };
    if let Some(config) = &mut report.pressure {
        config.outcome = Some(outcome);
    }
    eprintln!(
        "{} {label}: {}; admitted={}",
        if passed { "PASS" } else { "FAIL" },
        outcome.name(),
        report.achieved()
    );
    fs::write(
        directory.join("validated.json"),
        serde_json::to_vec_pretty(&[&report])?,
    )?;
    print_comparison(std::slice::from_ref(&report));
    Ok((report, passed))
}

pub fn run(args: Args) -> Result<()> {
    let (cancel, interrupted) = mpsc::channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    std::thread::spawn(move || {
        if runtime.block_on(termination()).is_ok() {
            let _ = cancel.send(());
        }
    });
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    fs::create_dir_all(&args.target_dir)?;
    let target = args.target_dir.canonicalize()?;
    let density = target.join("density");
    fs::create_dir_all(&density)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let artifacts = density.join(format!("memory_pressure-{stamp}"));
    fs::create_dir(&artifacts)?;
    eprintln!("pressure artifacts: {}", artifacts.display());
    let (image, binary) = build(&args, &root, &artifacts)?;
    fs::write(
        artifacts.join("source.patch"),
        output(
            Command::new("git")
                .args(["diff", "--binary", "HEAD"])
                .current_dir(&root),
        )?,
    )?;
    fs::write(
        artifacts.join("manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "revision": output(Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&root))?,
            "dirty": !output(Command::new("git").args(["status", "--porcelain"]).current_dir(&root))?.is_empty(),
            "image": image, "binary": binary,
            "build": {"cpus": 4, "memory_mib": 8192}, "measured": {"cpus": 2, "swap_mib": 0},
        }))?,
    )?;
    let mut reports = Vec::new();
    let mut failed = 0;
    for &memory_mib in &args.memory_mib {
        for &shape in &args.shapes {
            for repeat in 1..=args.repeats {
                for cow in &args.cow {
                    for mode in &args.modes {
                        let case = Case {
                            memory_mib,
                            shape,
                            repeat: repeat as usize,
                            cow: cow == "on",
                            mode: mode.clone(),
                            dirty_mib: args.dirty_mib,
                            guest_memory_mib: args.guest_memory_mib,
                            initial_cap_mib: args.initial_cap_mib,
                        };
                        ensure!(interrupted.try_recv().is_err(), "pressure run interrupted");
                        let (report, passed) =
                            run_case(&args, &artifacts, &image, &binary, &case, &interrupted)?;
                        failed += usize::from(!passed);
                        reports.push(report);
                        fs::write(
                            density.join("memory_pressure.json"),
                            serde_json::to_vec_pretty(&reports)?,
                        )?;
                    }
                }
            }
        }
    }
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

async fn termination() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
