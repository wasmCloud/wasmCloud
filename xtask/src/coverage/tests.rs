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
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/coverage_tool.rs");
        let status = Command::new("rustc")
            .args(["--edition=2024", "--crate-name", "coverage_tool"])
            .arg(source)
            .arg("-o")
            .arg(&tool)
            .status()?;
        anyhow::ensure!(status.success(), "failed to build the test tool");
        for name in ["cargo", "git", "rustc"] {
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
            rustc: program("rustc"),
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
            .filter(|line| line.starts_with("cargo\tllvm-cov\treport"))
            .all(|line| line.contains(&filter))
    );
    let provenance = fs::read_to_string(runner.report_dir().join("provenance.txt"))?;
    assert!(provenance.contains("test-commit") && provenance.contains("Test exit status: 0"));
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
    for stage in ["test", "report", "check"] {
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
