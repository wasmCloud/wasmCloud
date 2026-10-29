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
// Generated protobuf bindings have no maintained application logic.
const REPORT_ARGS: &[&str] = &[
    "--ignore-filename-regex",
    r"[/\\]washlet[/\\]generated[/\\]",
];

#[derive(Subcommand)]
pub(crate) enum Task {
    /// Install the pinned coverage nightly and its matching LLVM tools.
    Setup,
    /// Collect native workspace coverage, including ignored tests.
    Test,
    /// Generate HTML, JSON, LCOV, and text reports.
    Report,
    /// Require passing tests and 80% line and compiler-region coverage.
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
                // Keep diagnostics when tests or the threshold check fail.
                self.report()?;
                tests?;
                self.check()
            }
        }
    }

    fn report_dir(&self) -> PathBuf {
        self.workspace.join("target/coverage")
    }

    fn command(&self, program: &OsString, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(self.workspace)
            .env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
            .env("CARGO_PROFILE_TEST_DEBUG", "line-tables-only")
            .env("CARGO_INCREMENTAL", "0");
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
        command.env_remove("RUSTC").env_remove("RUSTDOC");
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
            "Scope: native Rust workspace; generated protobuf bindings excluded"
        )?;
        if let Ok(id) = std::env::var("GITHUB_RUN_ID") {
            let server = std::env::var("GITHUB_SERVER_URL")?;
            let repository = std::env::var("GITHUB_REPOSITORY")?;
            writeln!(provenance, "Run: {server}/{repository}/actions/runs/{id}")?;
        }
        self.cargo(&["llvm-cov", "clean", "--workspace"], None)?;
        // Build guests before cargo-llvm-cov enables native instrumentation.
        self.cargo(&["xtask", "build-fixtures"], None)?;
        let tests = self.cargo(
            &[
                "llvm-cov",
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
        for format in [
            vec![
                "--json",
                "--summary-only",
                "--output-path",
                "target/coverage/coverage.json",
            ],
            vec!["--lcov", "--output-path", "target/coverage/lcov.info"],
            vec!["--html", "--output-dir", "target/coverage"],
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
        let mut args = vec!["llvm-cov", "report"];
        args.extend_from_slice(REPORT_ARGS);
        args.extend(["--fail-under-lines", "80", "--fail-under-regions", "80"]);
        self.cargo(&args, Some("threshold.txt"))
    }
}

fn execute(mut command: Command, mut log: Option<File>, echo: bool) -> Result<()> {
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
            let output = (|| -> io::Result<()> {
                for message in receiver {
                    let (is_stderr, bytes) = message?;
                    if let Some(log) = &mut log {
                        log.write_all(&bytes)?;
                    }
                    if echo {
                        if is_stderr {
                            io::stderr().write_all(&bytes)?;
                        } else {
                            io::stdout().write_all(&bytes)?;
                        }
                    }
                }
                Ok(())
            })();
            if output.is_err() {
                let _ = child.kill();
            }
            let status = child.wait()?;
            output?;
            Ok(status)
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

    struct Fixture {
        root: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Result<Self> {
            let root = tempfile::Builder::new()
                .prefix("coverage runner ")
                .tempdir()?;
            let bin = root.path().join("bin");
            fs::create_dir(&bin)?;
            let tool = bin
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
                fs::copy(
                    &tool,
                    bin.join(name)
                        .with_extension(std::env::consts::EXE_EXTENSION),
                )?;
            }
            Ok(Self { root })
        }

        fn runner(&self) -> Runner<'_> {
            let program = |name| {
                self.root
                    .path()
                    .join("bin")
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
            "lcov.info",
            "html/index.html",
            "threshold.txt",
        ] {
            assert!(runner.report_dir().join(file).exists(), "{file}");
        }
        let calls = fixture.calls()?;
        let cwd = format!(
            "cwd={}\tdebug=line-tables-only\tincremental=0",
            fs::canonicalize(fixture.root.path())?.display()
        );
        assert!(calls.lines().all(|line| line.ends_with(&cwd)));
        let build = calls
            .find("xtask\tbuild-fixtures")
            .context("missing fixture build")?;
        let tests = calls.find("--no-report").context("missing test command")?;
        assert!(build < tests);
        assert!(calls.contains(
            "--locked\t--workspace\t--features\twash-runtime/wasi-tls,wash/host-component-plugins"
        ));
        assert!(calls.contains("--no-report\t--no-fail-fast\t--\t--include-ignored"));
        assert!(calls.contains("--fail-under-lines\t80\t--fail-under-regions\t80"));
        let filter = format!(
            "--ignore-filename-regex\t{}",
            r"[/\\]washlet[/\\]generated[/\\]"
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
        Ok(())
    }

    #[test]
    fn setup_installs_only_the_pinned_coverage_toolchain() -> Result<()> {
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
    fn report_failures_propagate_without_checking_thresholds() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.scenario("TEST_EXIT=7\nREPORT_EXIT=12")?;
        assert_eq!(failure_code(fixture.runner().run(None))?, 12);
        assert!(!fixture.calls()?.contains("--fail-under-lines"));
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
