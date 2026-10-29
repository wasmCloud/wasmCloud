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

const FEATURES: &str = "wash-runtime/wasi-tls,wash/host-component-plugins";
// Generated protobuf bindings have no maintained application logic.
const REPORT_ARGS: &[&str] = &[
    "--ignore-filename-regex",
    r"[/\\]washlet[/\\]generated[/\\]",
];

#[derive(Subcommand)]
pub(crate) enum Task {
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
        rustc: std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()),
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
    rustc: OsString,
}

impl Runner<'_> {
    fn run(&self, task: Option<Task>) -> Result<()> {
        match task {
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
        execute(self.command(&self.cargo, args), file, true)
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
            (&self.rustc, vec!["-vV"]),
            (&self.cargo, vec!["llvm-cov", "--version"]),
        ] {
            let log = OpenOptions::new().append(true).open(&provenance_path)?;
            execute(self.command(program, &args), Some(log), false)?;
        }
        let mut provenance = OpenOptions::new().append(true).open(&provenance_path)?;
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
mod tests;
