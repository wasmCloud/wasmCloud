//! Exercise orchestration with a fake Docker executable, without containers.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, ensure};
use bench_tools::density::pressure::{Config, MIB, Outcome, Phase, Sample, Shape};
use bench_tools::density::{Report, Step};
use serde_json::json;

const DOCKER: &str = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$PRESSURE_FIXTURE/calls"
case "$1" in
  run)
    for arg in "$@"; do
      case "$arg" in --cidfile=*) printf '%064d\n' 1 > "${arg#--cidfile=}";; esac
    done
    printf '%s\n' "$$" > "$PRESSURE_FIXTURE/build-started"
    if [ "${HANG_BUILD:-0}" = 1 ]; then exec sleep 300; fi
    if [ "${FAIL_BUILD:-0}" = 1 ]; then exit 23; fi
    printf '%s\n' '{"target":{"name":"memory_pressure"},"executable":"/target/release/deps/memory_pressure-fixture"}'
    ;;
  image) printf '%s\n' sha256:fixture;;
  create)
    printf '%s\n' "$@" > "$PRESSURE_FIXTURE/create-args"
    for arg in "$@"; do
      case "$arg" in type=bind,src=*,dst=/results) path=${arg#type=bind,src=}; printf '%s' "${path%,dst=/results}" > "$PRESSURE_FIXTURE/report-dir";; esac
    done
    printf '%064d\n' 2
    ;;
  start)
    cp "$PRESSURE_FIXTURE/input.json" "$(cat "$PRESSURE_FIXTURE/report-dir")/report.json"
    printf '%s\n' "$$" > "$PRESSURE_FIXTURE/started"
    if [ "${HANG_CASE:-0}" = 1 ]; then exec sleep 300; fi
    ;;
  inspect) cat "$PRESSURE_FIXTURE/exit.json";;
  kill|rm)
    printf '%s\n' "$*" >> "$PRESSURE_FIXTURE/cleanup"
    if [ -f "$PRESSURE_FIXTURE/started" ]; then kill "$(cat "$PRESSURE_FIXTURE/started")" 2>/dev/null || :; fi
    ;;
  *) exit 24;;
esac
"#;

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new(shape: Shape, mode: &str, oom: bool) -> Result<Self> {
        let fixture = Self(tempfile::tempdir()?);
        fs::write(fixture.path().join("docker"), DOCKER)?;
        fs::set_permissions(
            fixture.path().join("docker"),
            fs::Permissions::from_mode(0o755),
        )?;
        let admitted = if mode == "growth" { 2304 } else { 576 };
        let phases = if oom {
            vec![Phase::Baseline, Phase::Ready, Phase::Writing]
        } else {
            vec![
                Phase::Baseline,
                Phase::Ready,
                Phase::Writing,
                Phase::Touched,
                Phase::Released,
            ]
        };
        let config = Config {
            shape,
            cow: true,
            memories: 1,
            initial_bytes: MIB,
            swap_limit: 0,
            guest_limit: 576 * MIB,
            initial_limit: 2304 * MIB,
            process_limit: 691 * MIB,
            architecture: "fixture".into(),
            kernel: None,
            transparent_hugepage: None,
            outcome: None,
        };
        let steps = phases
            .into_iter()
            .map(|phase| {
                let count = if matches!(phase, Phase::Baseline | Phase::Released) {
                    0
                } else {
                    admitted
                };
                Step {
                    workloads: count,
                    guest_memory_bytes: Some(if mode == "enforce" {
                        count as u64 * MIB
                    } else {
                        0
                    }),
                    initial_memory_bytes: Some(if mode == "growth" {
                        count as u64 * MIB
                    } else {
                        0
                    }),
                    clean: phase == Phase::Touched,
                    pressure: Some(Sample {
                        phase,
                        cgroup_current: Some(256 * MIB),
                        cgroup_peak: Some(300 * MIB),
                        guest_refused: u64::from(mode == "enforce"),
                        initial_refused: u64::from(mode == "growth"),
                        process_refused: 0,
                        process_over: false,
                    }),
                    ..Default::default()
                }
            })
            .collect();
        let report = Report {
            bench: "memory_pressure".into(),
            label: format!("768m-{}-{mode}-cow-true-1", shape.name()),
            subject: shape.name().into(),
            guest_memory_mode: Some(mode.into()),
            memory_limit_bytes: 768 * MIB,
            pressure: Some(config),
            stop: "fixture admission refusal".into(),
            steps,
            ..Default::default()
        };
        fs::write(
            fixture.path().join("input.json"),
            serde_json::to_vec(&[report])?,
        )?;
        fixture.exit(if oom { 137 } else { 0 }, oom)?;
        Ok(fixture)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
    fn target(&self) -> PathBuf {
        self.path().join("target")
    }

    fn exit(&self, code: i32, oom: bool) -> Result<()> {
        fs::write(
            self.path().join("exit.json"),
            serde_json::to_vec(&json!([{"State":{"ExitCode":code,"OOMKilled":oom}}]))?,
        )?;
        Ok(())
    }

    fn command(&self, shape: Shape, mode: &str) -> Result<Command> {
        let mut paths = vec![self.path().to_owned()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let mut command = Command::new(env!("CARGO_BIN_EXE_bench-tools"));
        command
            .args([
                "memory-pressure",
                "--shapes",
                shape.name(),
                "--modes",
                mode,
                "--cow",
                "on",
                "--target-dir",
            ])
            .arg(self.target())
            .env("PATH", std::env::join_paths(paths)?)
            .env("PRESSURE_FIXTURE", self.path());
        Ok(command)
    }

    fn report(&self) -> Result<Report> {
        let reports: Vec<Report> = serde_json::from_slice(&fs::read(
            self.target().join("density/memory_pressure.json"),
        )?)?;
        reports
            .into_iter()
            .next()
            .context("missing aggregate report")
    }

    fn signal(&self, child: &mut ChildGuard, event: &str) -> Result<()> {
        wait_until(
            || {
                Ok(fs::read_to_string(self.path().join(event))
                    .ok()
                    .is_some_and(|pid| pid.trim().parse::<u32>().is_ok()))
            },
            "Docker phase never started",
        )
        .with_context(|| fs::read_to_string(self.path().join("driver.log")).unwrap_or_default())?;
        ensure!(
            Command::new("kill")
                .args(["-TERM", &child.0.id().to_string()])
                .status()?
                .success(),
            "signal failed"
        );
        wait_until(
            || Ok(child.0.try_wait()?.is_some()),
            "driver ignored SIGTERM",
        )
    }

    fn removed(&self, id: u64) -> Result<bool> {
        Ok(fs::read_to_string(self.path().join("cleanup"))?
            .lines()
            .any(|line| line == format!("rm --force {id:064x}")))
    }
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_until(mut condition: impl FnMut() -> Result<bool>, message: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition()? {
        ensure!(Instant::now() < deadline, "{message}");
        std::thread::park_timeout(Duration::from_millis(10));
    }
    Ok(())
}

fn passed(output: &Output) -> Result<()> {
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn completed_case_applies_limits_cleans_up_and_feeds_shared_reports() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    passed(&fixture.command(Shape::Sparse, "growth")?.output()?)?;
    let report = fixture.report()?;
    assert_eq!(
        report.pressure.as_ref().and_then(|p| p.outcome),
        Some(Outcome::Completed)
    );
    assert_eq!(report.achieved(), 2304);
    let args = fs::read_to_string(fixture.path().join("create-args"))?;
    for setting in [
        "--memory=768m",
        "--memory-swap=768m",
        "--cpus=2",
        "--network=none",
        "--env=WASH_PRESSURE_COW=true",
        "--bench",
    ] {
        assert!(
            args.lines().any(|line| line == setting),
            "missing {setting}"
        );
    }
    assert!(fixture.removed(2)?);
    for subcommand in ["jsonl", "summary"] {
        let output = Command::new(env!("CARGO_BIN_EXE_bench-tools"))
            .args([subcommand, "--bench", "memory_pressure", "--target-dir"])
            .arg(fixture.target())
            .output()?;
        passed(&output)?;
        assert!(String::from_utf8(output.stdout)?.contains("admitted_stores"));
    }
    Ok(())
}

#[test]
fn verified_oom_passes_and_unverified_exit_137_fails() -> Result<()> {
    for verified in [true, false] {
        let fixture = Fixture::new(Shape::Dirty, "growth", true)?;
        fixture.exit(137, verified)?;
        let output = fixture.command(Shape::Dirty, "growth")?.output()?;
        assert_eq!(output.status.success(), verified);
        assert_eq!(
            fixture.report()?.pressure.and_then(|p| p.outcome),
            Some(if verified {
                Outcome::ExpectedOom
            } else {
                Outcome::Failed
            })
        );
        assert!(fixture.removed(2)?);
    }
    Ok(())
}

#[test]
fn malformed_checkpoint_still_removes_the_container() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    fs::write(fixture.path().join("input.json"), "{broken")?;
    let output = fixture.command(Shape::Sparse, "growth")?.output()?;
    assert!(!output.status.success());
    assert!(fixture.removed(2)?);
    Ok(())
}

#[test]
fn timeout_fails_even_with_an_oom_flag_and_complete_checkpoint() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    fixture.exit(137, true)?;
    let output = fixture
        .command(Shape::Sparse, "growth")?
        .args(["--timeout", "1"])
        .env("HANG_CASE", "1")
        .output()?;
    assert!(!output.status.success());
    assert!(fixture.report()?.stop.contains("timed out"));
    let cleanup = fs::read_to_string(fixture.path().join("cleanup"))?;
    assert!(cleanup.lines().any(|line| line.starts_with("kill ")));
    assert!(fixture.removed(2)?);
    Ok(())
}

#[test]
fn sigterm_during_measurement_removes_the_container() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    let mut child = ChildGuard(
        fixture
            .command(Shape::Sparse, "growth")?
            .env("HANG_CASE", "1")
            .stdout(Stdio::null())
            .stderr(fs::File::create(fixture.path().join("driver.log"))?)
            .spawn()?,
    );
    fixture.signal(&mut child, "started")?;
    assert!(fixture.removed(2)?);
    Ok(())
}

#[test]
fn sigterm_during_build_stops_the_build_and_cleans_up() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    let mut child = ChildGuard(
        fixture
            .command(Shape::Sparse, "growth")?
            .env("HANG_BUILD", "1")
            .stdout(Stdio::null())
            .stderr(fs::File::create(fixture.path().join("driver.log"))?)
            .spawn()?,
    );
    let result = fixture.signal(&mut child, "build-started");
    // A failed regression check must not leave its fake build process running.
    if result.is_err()
        && let Ok(pid) = fs::read_to_string(fixture.path().join("build-started"))
    {
        let _ = Command::new("kill").args(["-KILL", pid.trim()]).status();
    }
    result?;
    assert!(fixture.removed(1)?);
    Ok(())
}

#[test]
fn a_failed_new_build_cannot_leave_the_previous_aggregate_as_current() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    let aggregate = fixture.target().join("density/memory_pressure.json");
    fs::create_dir_all(aggregate.parent().context("missing report directory")?)?;
    fs::copy(fixture.path().join("input.json"), &aggregate)?;
    let output = fixture
        .command(Shape::Sparse, "growth")?
        .env("FAIL_BUILD", "1")
        .output()?;
    assert!(!output.status.success());
    assert!(
        !aggregate.exists(),
        "previous run is still advertised as the current report"
    );
    Ok(())
}

#[test]
fn saved_source_patch_keeps_exact_bytes_and_is_parseable() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    let patch = b"diff --git a/pressure-fixture.txt b/pressure-fixture.txt\nnew file mode 100644\n--- /dev/null\n+++ b/pressure-fixture.txt\n@@ -0,0 +1 @@\n+fixture content\n";
    fs::write(fixture.path().join("source.diff"), patch)?;
    let real_git = Command::new("sh").args(["-c", "command -v git"]).output()?;
    passed(&real_git)?;
    let real_git = String::from_utf8(real_git.stdout)?;
    let git = fixture.path().join("git");
    fs::write(
        &git,
        "#!/bin/sh\nif [ \"$1\" = diff ]; then cat \"$PRESSURE_FIXTURE/source.diff\"; else exec \"$PRESSURE_REAL_GIT\" \"$@\"; fi\n",
    )?;
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755))?;
    passed(
        &fixture
            .command(Shape::Sparse, "growth")?
            .env("PRESSURE_REAL_GIT", real_git.trim())
            .output()?,
    )?;
    let artifacts = fs::read_dir(fixture.target().join("density"))?
        .filter_map(Result::ok)
        .find(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .context("missing artifacts")?
        .path();
    let saved = artifacts.join("source.patch");
    assert_eq!(fs::read(&saved)?, patch);
    passed(
        &Command::new(real_git.trim())
            .args(["apply", "--numstat"])
            .arg(saved)
            .output()?,
    )?;
    Ok(())
}
