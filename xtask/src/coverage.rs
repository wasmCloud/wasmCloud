use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Subcommand;

const TOOLCHAIN: &str = include_str!("../coverage-toolchain");

const FEATURES: &str = "wash-runtime/wasi-tls,wash/host-component-plugins";
// Build tooling and generated protobuf bindings are outside the coverage scope.
const REPORT_ARGS: &[&str] = &[
    "--ignore-filename-regex",
    r"[/\\](washlet[/\\]generated|xtask)[/\\]",
];
/// Minimum line coverage percentage.
const THRESHOLD: u8 = 80;
/// Report output, relative to the workspace root.
const REPORT_DIR: &str = "target/coverage";
const JSON_SUMMARY: &str = "coverage.json";
/// `llvm-cov` summary keys and the names used in threshold failures.
const METRICS: [(&str, &str); 1] = [("lines", "line")];

#[derive(Subcommand)]
pub(crate) enum Task {
    /// Install the pinned coverage nightly and its matching LLVM tools.
    Setup,
    /// Collect native workspace coverage, including ignored tests.
    Test,
    /// Generate HTML, JSON, and text reports.
    Report,
    #[command(about = format!(
        "Require passing tests and {THRESHOLD}% line coverage."
    ))]
    Check,
}

#[derive(Debug)]
struct CommandFailure {
    command: OsString,
    status: ExitStatus,
}

impl CommandFailure {
    fn code(&self) -> i32 {
        if let Some(code) = self.status.code() {
            return code;
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            if let Some(signal) = self.status.signal() {
                return 128 + signal;
            }
        }
        1
    }
}

impl std::fmt::Display for CommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} failed ({})",
            self.command.to_string_lossy(),
            self.status
        )
    }
}

impl std::error::Error for CommandFailure {}

pub(crate) fn run(workspace: &Path, task: Option<Task>) -> Result<()> {
    let result = Runner {
        workspace,
        cargo: std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
        git: "git".into(),
        rustup: "rustup".into(),
    }
    .run(task);
    if let Err(error) = &result
        && let Some(failure) = error.downcast_ref::<CommandFailure>()
    {
        eprintln!("Error: {error:#}");
        std::process::exit(failure.code());
    }
    result
}

struct Runner<'a> {
    workspace: &'a Path,
    cargo: OsString,
    git: OsString,
    rustup: OsString,
}

impl Runner<'_> {
    fn run(&self, task: Option<Task>) -> Result<()> {
        match task {
            Some(Task::Setup) => execute(
                self.command(
                    &self.rustup,
                    &[
                        "toolchain",
                        "install",
                        TOOLCHAIN.trim(),
                        "--profile",
                        "minimal",
                        "--component",
                        "llvm-tools-preview",
                    ],
                ),
                None,
                true,
            ),
            Some(Task::Test) => self.collect(),
            Some(Task::Report) => self.report(),
            Some(Task::Check) => self.check(),
            None => {
                let tests = self.collect();
                // Keep diagnostics when tests or the threshold check fail, but
                // report the first failure.
                let report = self.report();
                match (tests, report) {
                    (Err(tests), Err(report)) => {
                        eprintln!("Error: coverage report also failed: {report:#}");
                        Err(tests)
                    }
                    (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
                    (Ok(()), Ok(())) => self.check(),
                }
            }
        }
    }

    fn report_dir(&self) -> PathBuf {
        self.workspace.join(REPORT_DIR)
    }

    fn command(&self, program: &OsString, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command.args(args).current_dir(self.workspace);
        command
    }

    fn cargo(&self, args: &[&str], log: Option<&str>) -> Result<()> {
        let file = log
            .map(|name| File::create(self.report_dir().join(name)))
            .transpose()?;
        let command = if args.first() == Some(&"llvm-cov") {
            self.coverage_command("cargo", args)
        } else {
            self.command(&self.cargo, args)
        };
        execute(command, file, true)
    }

    fn coverage_command(&self, program: &str, args: &[&str]) -> Command {
        let mut invocation = vec!["run", TOOLCHAIN.trim(), program];
        invocation.extend_from_slice(args);
        let mut command = self.command(&self.rustup, &invocation);
        // Only instrumented builds get these: the nested `cargo xtask` would
        // otherwise rebuild the running xtask under a different profile.
        command
            .env_remove("RUSTC")
            .env_remove("RUSTDOC")
            .env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
            .env("CARGO_PROFILE_TEST_DEBUG", "line-tables-only")
            .env("CARGO_INCREMENTAL", "0");
        command
    }

    fn collect(&self) -> Result<()> {
        let dir = self.report_dir();
        match fs::remove_dir_all(&dir) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        fs::create_dir_all(&dir)?;
        let provenance_path = dir.join("provenance.txt");
        let mut provenance = File::create(&provenance_path)?;
        writeln!(
            provenance,
            "Collected (Unix seconds, UTC): {}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
        )?;
        drop(provenance);
        for (program, args) in [
            (&self.git, vec!["rev-parse", "HEAD"]),
            (&self.git, vec!["status", "--short"]),
        ] {
            let log = OpenOptions::new().append(true).open(&provenance_path)?;
            execute(self.command(program, &args), Some(log), false)?;
        }
        for (program, args) in [
            ("rustc", vec!["-vV"]),
            ("cargo", vec!["llvm-cov", "--version"]),
        ] {
            let log = OpenOptions::new().append(true).open(&provenance_path)?;
            execute(self.coverage_command(program, &args), Some(log), false)?;
        }
        let mut provenance = OpenOptions::new().append(true).open(&provenance_path)?;
        writeln!(provenance, "Coverage toolchain: {}", TOOLCHAIN.trim())?;
        writeln!(provenance, "Features: {FEATURES}")?;
        writeln!(
            provenance,
            "Tests: workspace unit and integration tests, including ignored tests"
        )?;
        writeln!(
            provenance,
            "Scope: native Rust workspace; xtask and generated protobuf bindings excluded"
        )?;
        let thresholds = METRICS.map(|(key, _)| format!("{THRESHOLD}% {key}"));
        writeln!(provenance, "Thresholds: {}", thresholds.join(", "))?;
        if let (Ok(server), Ok(repository), Ok(id)) = (
            std::env::var("GITHUB_SERVER_URL"),
            std::env::var("GITHUB_REPOSITORY"),
            std::env::var("GITHUB_RUN_ID"),
        ) {
            writeln!(provenance, "Run: {server}/{repository}/actions/runs/{id}")?;
        }
        self.cargo(&["llvm-cov", "clean", "--workspace"], None)?;
        // Build guests before cargo-llvm-cov enables native instrumentation.
        self.cargo(&["xtask", "build-fixtures"], None)?;
        let tests = self.cargo(
            &[
                "llvm-cov",
                "--branch",
                "--locked",
                "--workspace",
                "--features",
                FEATURES,
                "--no-report",
                "--no-fail-fast",
                "--",
                "--include-ignored",
            ],
            Some("tests.txt"),
        );
        let status = tests.as_ref().err().map_or(0, |error| {
            error
                .downcast_ref::<CommandFailure>()
                .map_or(1, CommandFailure::code)
        });
        writeln!(provenance, "Test exit status: {status}")?;
        tests?;
        fs::write(dir.join("tests-passed"), "")?;
        Ok(())
    }

    fn report(&self) -> Result<()> {
        fs::create_dir_all(self.report_dir())?;
        let mut args = vec!["llvm-cov", "report"];
        args.extend_from_slice(REPORT_ARGS);
        self.cargo(&args, Some("summary.txt"))?;
        let json = format!("{REPORT_DIR}/{JSON_SUMMARY}");
        // No LCOV: the pinned nightly's llvm-cov segfaults exporting branch data as LCOV.
        for format in [
            vec!["--json", "--summary-only", "--output-path", json.as_str()],
            vec!["--html", "--output-dir", REPORT_DIR],
        ] {
            let mut command = args.clone();
            command.extend(format);
            self.cargo(&command, None)?;
        }
        Ok(())
    }

    fn check(&self) -> Result<()> {
        anyhow::ensure!(
            self.report_dir().join("tests-passed").is_file(),
            "Coverage evidence requires a successful full test run."
        );
        let flags = METRICS.map(|(key, _)| format!("--fail-under-{key}"));
        let threshold = THRESHOLD.to_string();
        let mut args = vec!["llvm-cov", "report"];
        args.extend_from_slice(REPORT_ARGS);
        for flag in &flags {
            args.extend([flag.as_str(), threshold.as_str()]);
        }
        match self.cargo(&args, Some("threshold.txt")) {
            Ok(()) => Ok(()),
            // llvm-cov exits 1 without naming the metric, so explain it from the
            // summary, which `collect()` removes whenever it gathers new data.
            Err(error) => match shortfalls(&self.report_dir().join(JSON_SUMMARY)) {
                Ok(Some(message)) => Err(error.context(message)),
                _ => Err(error),
            },
        }
    }
}

/// Describes each metric in the JSON summary that is below [`THRESHOLD`].
fn shortfalls(summary: &Path) -> Result<Option<String>> {
    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(summary).context("Failed to read coverage summary")?)?;
    let data = summary
        .get("data")
        .and_then(serde_json::Value::as_array)
        .filter(|data| !data.is_empty())
        .context("Coverage summary has no data")?;
    let mut failures = Vec::new();
    for (key, name) in METRICS {
        // The same arithmetic as cargo-llvm-cov's `--fail-under-*` checks.
        let (mut count, mut covered) = (0.0, 0.0);
        for entry in data {
            let totals = &entry["totals"][key];
            count += totals["count"]
                .as_f64()
                .with_context(|| format!("Coverage summary is missing {key} count"))?;
            covered += totals["covered"]
                .as_f64()
                .with_context(|| format!("Coverage summary is missing {key} covered count"))?;
        }
        let percent = if count == 0.0 {
            0.0
        } else {
            covered * 100.0 / count
        };
        if percent < f64::from(THRESHOLD) {
            // Truncate so a value just under the bar never prints as equal to it.
            let shown = (percent * 100.0).floor() / 100.0;
            failures.push(format!("{name} coverage {shown:.2}% is below {THRESHOLD}%"));
        }
    }
    Ok((!failures.is_empty()).then(|| failures.join("; ")))
}

fn execute(mut command: Command, mut log: Option<File>, mut echo: bool) -> Result<()> {
    let program = command.get_program().to_owned();
    let status = if log.is_some() {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().context("missing child stdout")?;
        let stderr = child.stderr.take().context("missing child stderr")?;
        thread::scope(|scope| -> Result<ExitStatus> {
            let (sender, receiver) = mpsc::sync_channel(16);
            for (stream, is_stderr) in [
                (Box::new(stdout) as Box<dyn Read + Send>, false),
                (Box::new(stderr) as Box<dyn Read + Send>, true),
            ] {
                let sender = sender.clone();
                scope.spawn(move || forward(stream, is_stderr, sender));
            }
            drop(sender);
            // A failed write stops only that output and is reported after the
            // child exits. Killing the child would not stop the processes it
            // started, which hold the pipes open.
            let mut failure = None;
            for message in receiver {
                let (is_stderr, bytes) = match message {
                    Ok(message) => message,
                    Err(error) => {
                        failure.get_or_insert(error);
                        continue;
                    }
                };
                if let Some(file) = log.as_mut()
                    && let Err(error) = file.write_all(&bytes)
                {
                    failure.get_or_insert(error);
                    log = None;
                }
                if echo {
                    let written = if is_stderr {
                        io::stderr().write_all(&bytes)
                    } else {
                        io::stdout().write_all(&bytes)
                    };
                    if let Err(error) = written {
                        failure.get_or_insert(error);
                        echo = false;
                    }
                }
            }
            let status = child.wait()?;
            match failure {
                Some(error) => Err(error.into()),
                None => Ok(status),
            }
        })?
    } else {
        command.status()?
    };
    if !status.success() {
        return Err(CommandFailure {
            command: program,
            status,
        }
        .into());
    }
    Ok(())
}

fn forward(
    mut stream: Box<dyn Read + Send>,
    is_stderr: bool,
    sender: mpsc::SyncSender<io::Result<(bool, Vec<u8>)>>,
) {
    let mut buffer = [0; 8192];
    loop {
        let message = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => Ok((is_stderr, buffer[..count].to_vec())),
            Err(error) => {
                let _ = sender.send(Err(error));
                break;
            }
        };
        if sender.send(message).is_err() {
            break;
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use clap::Parser as _;
    use std::sync::OnceLock;

    /// The fake `cargo`, `git`, and `rustup`, compiled once per test run.
    fn tools() -> Result<&'static Path> {
        static TOOL: OnceLock<Result<PathBuf, String>> = OnceLock::new();
        let build = || -> Result<PathBuf> {
            let executable = std::env::current_exe()?;
            let cache = executable
                .parent()
                .context("missing test executable directory")?
                .join("coverage-test-tools");
            fs::create_dir_all(&cache)?;
            let dir = tempfile::Builder::new().prefix("tool-").tempdir_in(cache)?;
            let tool = dir
                .path()
                .join("tool")
                .with_extension(std::env::consts::EXE_EXTENSION);
            let source =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/coverage_tool.rs");
            let status = Command::new("rustc")
                .args(["--edition=2024", "--crate-name", "coverage_tool"])
                .arg(source)
                .arg("-o")
                .arg(&tool)
                .status()?;
            anyhow::ensure!(status.success(), "failed to build the test tool");
            for name in ["cargo", "git", "rustup"] {
                let path = dir
                    .path()
                    .join(name)
                    .with_extension(std::env::consts::EXE_EXTENSION);
                // Never open executables for writing while other tests spawn children.
                fs::hard_link(&tool, path)?;
            }
            // `cargo clean` reclaims helpers; each run gets an immutable executable.
            Ok(dir.keep())
        };
        TOOL.get_or_init(|| build().map_err(|error| format!("{error:#}")))
            .as_deref()
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    struct Fixture {
        root: tempfile::TempDir,
        bin: &'static Path,
    }

    impl Fixture {
        fn new() -> Result<Self> {
            let bin = tools()?;
            let root = tempfile::Builder::new()
                .prefix("coverage runner ")
                .tempdir()?;
            Ok(Self { root, bin })
        }

        fn runner(&self) -> Runner<'_> {
            let program = |name| {
                self.bin
                    .join(name)
                    .with_extension(std::env::consts::EXE_EXTENSION)
                    .into_os_string()
            };
            Runner {
                workspace: self.root.path(),
                cargo: program("cargo"),
                git: program("git"),
                rustup: program("rustup"),
            }
        }

        fn scenario(&self, contents: &str) -> Result<()> {
            fs::write(self.root.path().join("scenario"), contents)?;
            Ok(())
        }

        fn calls(&self) -> Result<String> {
            Ok(fs::read_to_string(self.root.path().join("calls.txt"))?)
        }
    }

    fn failure_code(result: Result<()>) -> Result<i32> {
        let error = result.err().context("expected a failed command")?;
        Ok(error
            .downcast_ref::<CommandFailure>()
            .context("expected a subprocess failure")?
            .code())
    }

    #[test]
    fn all_stages_produce_complete_evidence_with_the_expected_scope() -> Result<()> {
        let fixture = Fixture::new()?;
        let runner = fixture.runner();
        runner.run(None)?;
        for file in [
            "tests-passed",
            "tests.txt",
            "summary.txt",
            "coverage.json",
            "html/index.html",
            "threshold.txt",
        ] {
            assert!(runner.report_dir().join(file).exists(), "{file}");
        }
        let calls = fixture.calls()?;
        let cwd = format!("cwd={}\t", fs::canonicalize(fixture.root.path())?.display());
        assert!(calls.lines().all(|line| line.contains(&cwd)));
        let instrumented = "\tdebug=line-tables-only\tincremental=0";
        let inherited = |key| std::env::var(key).unwrap_or_else(|_| "unset".into());
        let inherited = format!(
            "\tdebug={}\tincremental={}",
            inherited("CARGO_PROFILE_TEST_DEBUG"),
            inherited("CARGO_INCREMENTAL")
        );
        for line in calls.lines() {
            let expected = if line.starts_with("rustup\trun\t") {
                instrumented
            } else {
                &inherited
            };
            assert!(line.ends_with(expected), "{line}");
        }
        let build = calls
            .find("xtask\tbuild-fixtures")
            .context("missing fixture build")?;
        let tests = calls.find("--no-report").context("missing test command")?;
        assert!(build < tests);
        assert!(calls.contains(
            "--branch\t--locked\t--workspace\t--features\twash-runtime/wasi-tls,wash/host-component-plugins"
        ));
        assert!(calls.contains("--no-report\t--no-fail-fast\t--\t--include-ignored"));
        assert!(calls.contains("--fail-under-lines\t80"));
        assert!(!calls.contains("--fail-under-regions"));
        assert!(!calls.contains("--fail-under-branches"));
        let filter = format!(
            "--ignore-filename-regex\t{}",
            r"[/\\](washlet[/\\]generated|xtask)[/\\]"
        );
        assert!(
            calls
                .lines()
                .filter(|line| line.contains("\tcargo\tllvm-cov\treport"))
                .all(|line| line.contains(&filter))
        );
        let nightly = format!("rustup\trun\t{}\tcargo\tllvm-cov\t", TOOLCHAIN.trim());
        assert!(
            calls
                .lines()
                .filter(|line| line.contains("llvm-cov"))
                .all(|line| line.starts_with(&nightly))
        );
        assert!(
            calls
                .lines()
                .any(|line| line.starts_with("cargo\txtask\tbuild-fixtures"))
        );
        let provenance = fs::read_to_string(runner.report_dir().join("provenance.txt"))?;
        assert!(provenance.contains("test-commit") && provenance.contains("Test exit status: 0"));
        assert!(provenance.contains("Thresholds: 80% lines\n"));
        Ok(())
    }

    #[test]
    fn setup_installs_only_the_pinned_coverage_toolchain() -> Result<()> {
        assert!(TOOLCHAIN.trim().starts_with("nightly-"));
        let fixture = Fixture::new()?;
        fixture.runner().run(Some(Task::Setup))?;
        let calls = fixture.calls()?;
        assert!(calls.contains(&format!(
            "rustup\ttoolchain\tinstall\t{}\t--profile\tminimal\t--component\tllvm-tools-preview",
            TOOLCHAIN.trim()
        )));
        assert!(!calls.contains("default"));
        fixture.scenario("SETUP_EXIT=5")?;
        assert_eq!(failure_code(fixture.runner().run(Some(Task::Setup)))?, 5);
        Ok(())
    }

    #[test]
    fn failed_tests_preserve_diagnostics_without_passing_evidence() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.scenario("TEST_EXIT=7")?;
        let runner = fixture.runner();
        assert_eq!(failure_code(runner.run(None))?, 7);
        assert!(!runner.report_dir().join("tests-passed").exists());
        assert!(runner.report_dir().join("coverage.json").exists());
        let log = fs::read_to_string(runner.report_dir().join("tests.txt"))?;
        assert!(log.contains("test stdout") && log.contains("test stderr"));
        assert!(
            fs::read_to_string(runner.report_dir().join("provenance.txt"))?
                .contains("Test exit status: 7")
        );
        assert!(!fixture.calls()?.contains("--fail-under-lines"));
        Ok(())
    }

    #[test]
    fn fixture_failures_clear_stale_evidence_and_prevent_a_threshold_pass() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.scenario("FIXTURE_EXIT=23")?;
        let runner = fixture.runner();
        fs::create_dir_all(runner.report_dir())?;
        for file in ["tests-passed", "coverage.json"] {
            fs::write(runner.report_dir().join(file), "stale")?;
        }
        assert_eq!(failure_code(runner.run(Some(Task::Test)))?, 23);
        assert!(!runner.report_dir().join("tests-passed").exists());
        assert!(!runner.report_dir().join("coverage.json").exists());
        assert!(!fixture.calls()?.contains("--no-report"));
        assert!(runner.run(Some(Task::Check)).is_err());
        assert!(!fixture.calls()?.contains("--fail-under-lines"));
        Ok(())
    }

    #[test]
    fn threshold_failures_preserve_both_output_streams() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.scenario("THRESHOLD_EXIT=9")?;
        let runner = fixture.runner();
        assert_eq!(failure_code(runner.run(None))?, 9);
        let log = fs::read_to_string(runner.report_dir().join("threshold.txt"))?;
        assert!(log.contains("report stdout") && log.contains("report stderr"));
        Ok(())
    }

    #[test]
    fn line_threshold_failures_name_the_metric_below_the_bar() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.scenario("THRESHOLD_EXIT=1\nLINES_PERCENT=77.34\nREGIONS_PERCENT=81.48")?;
        let error = fixture
            .runner()
            .run(None)
            .err()
            .context("expected failure")?;
        assert_eq!(error.to_string(), "line coverage 77.34% is below 80%");
        assert!(format!("{error:#}").starts_with("line coverage 77.34% is below 80%: "));
        assert_eq!(failure_code(Err(error))?, 1);

        fixture.scenario(
            "THRESHOLD_EXIT=1\nLINES_PERCENT=79.999\nREGIONS_PERCENT=12.5\nBRANCHES_PERCENT=65",
        )?;
        let error = fixture
            .runner()
            .run(None)
            .err()
            .context("expected failure")?;
        assert_eq!(error.to_string(), "line coverage 79.99% is below 80%");

        // Without a summary to explain it, the llvm-cov failure is reported as is.
        let runner = fixture.runner();
        runner.run(Some(Task::Test))?;
        let error = runner
            .run(Some(Task::Check))
            .err()
            .context("expected failure")?;
        let failure = error
            .downcast_ref::<CommandFailure>()
            .context("expected a subprocess failure")?;
        assert_eq!(error.to_string(), failure.to_string());
        assert_eq!(failure_code(Err(error))?, 1);
        Ok(())
    }

    #[test]
    fn region_and_branch_shortfalls_do_not_fail_coverage() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.scenario("REGIONS_PERCENT=0\nBRANCHES_PERCENT=0")?;
        fixture.runner().run(None)?;
        Ok(())
    }

    #[test]
    fn missing_informational_metrics_do_not_fail_the_threshold() -> Result<()> {
        let fixture = Fixture::new()?;
        let runner = fixture.runner();
        runner.run(None)?;
        let path = runner.report_dir().join(JSON_SUMMARY);
        let mut summary: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
        let totals = summary["data"][0]["totals"]
            .as_object_mut()
            .context("missing totals")?;
        for metric in ["regions", "branches"] {
            totals.remove(metric);
        }
        fs::write(path, serde_json::to_vec(&summary)?)?;
        runner.run(Some(Task::Check))?;
        Ok(())
    }

    #[test]
    fn report_failures_propagate_without_hiding_earlier_failures() -> Result<()> {
        let fixture = Fixture::new()?;
        for (scenario, code) in [
            ("REPORT_EXIT=12", 12),
            ("TEST_EXIT=7\nREPORT_EXIT=12", 7),
            ("FIXTURE_EXIT=23", 23),
        ] {
            fixture.scenario(scenario)?;
            assert_eq!(
                failure_code(fixture.runner().run(None))?,
                code,
                "{scenario}"
            );
            assert!(!fixture.calls()?.contains("--fail-under-lines"));
        }
        Ok(())
    }

    #[test]
    fn log_write_failures_surface_after_the_command_finishes() -> Result<()> {
        let fixture = Fixture::new()?;
        let runner = fixture.runner();
        let path = fixture.root.path().join("read-only.txt");
        fs::write(&path, "")?;
        let command = runner.command(&runner.cargo, &["llvm-cov", "--no-report"]);
        let error = execute(command, Some(File::open(&path)?), false)
            .err()
            .context("expected a log failure")?;
        assert!(error.downcast_ref::<io::Error>().is_some(), "{error:#}");
        assert!(fixture.calls()?.contains("--no-report"));
        Ok(())
    }

    #[test]
    fn unwritable_test_logs_cannot_produce_passing_evidence() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.scenario("BLOCK_TEST_LOG=1")?;
        let runner = fixture.runner();
        assert!(runner.run(Some(Task::Test)).is_err());
        assert!(!runner.report_dir().join("tests-passed").exists());
        assert!(!fixture.calls()?.contains("--no-report"));
        Ok(())
    }

    #[test]
    fn coverage_cli_defaults_to_all_stages_and_rejects_unknown_stages() -> Result<()> {
        assert!(matches!(
            crate::Cli::try_parse_from(["xtask", "coverage"])?.task,
            crate::Task::Coverage { task: None }
        ));
        for stage in ["setup", "test", "report", "check"] {
            assert!(crate::Cli::try_parse_from(["xtask", "coverage", stage]).is_ok());
        }
        assert!(crate::Cli::try_parse_from(["xtask", "coverage", "unknown"]).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn signal_failures_preserve_the_exit_status() {
        use std::os::unix::process::ExitStatusExt as _;
        let failure = CommandFailure {
            command: "cargo".into(),
            status: ExitStatus::from_raw(15),
        };
        assert_eq!(failure.code(), 143);
    }
}
