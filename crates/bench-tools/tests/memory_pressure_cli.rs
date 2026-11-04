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
stall() {
  case " ${HANG_COMMANDS:-} " in
    *" $1 "*)
      printf '%s\n' "$$" >> "$PRESSURE_FIXTURE/hung-pids"
      printf '%s\n' "$$" > "$PRESSURE_FIXTURE/$1-started"
      exec sleep 300
      ;;
  esac
}
printf '%s\n' "$*" >> "$PRESSURE_FIXTURE/calls"
case "$1" in
  run)
    previous=
    for arg in "$@"; do
      case "$arg" in --cidfile=*) if [ "${OMIT_BUILD_CID:-0}" != 1 ]; then printf '%064d\n' 1 > "${arg#--cidfile=}"; fi;; esac
      if [ "$previous" = --name ]; then printf '%s\n' "$arg" > "$PRESSURE_FIXTURE/build-name"; fi
      previous=$arg
    done
    stall build
    if [ "${FAIL_BUILD:-0}" = 1 ]; then exit 23; fi
    printf '%s\n' '{"target":{"name":"memory_pressure"},"executable":"/target/release/deps/memory_pressure-fixture"}'
    ;;
  image) stall image; printf '%s\n' sha256:fixture;;
  create)
    printf '%s\n' "$@" > "$PRESSURE_FIXTURE/create-args"
    previous=
    for arg in "$@"; do
      case "$arg" in type=bind,src=*,dst=/results) path=${arg#type=bind,src=}; printf '%s' "${path%,dst=/results}" > "$PRESSURE_FIXTURE/report-dir";; esac
      if [ "$previous" = --name ]; then printf '%s\n' "$arg" > "$PRESSURE_FIXTURE/case-name"; fi
      previous=$arg
    done
    stall create
    printf '%064d\n' 2
    ;;
  start)
    cp "$PRESSURE_FIXTURE/input.json" "$(cat "$PRESSURE_FIXTURE/report-dir")/report.json"
    stall start
    ;;
  inspect) stall inspect; cat "$PRESSURE_FIXTURE/exit.json";;
  kill|rm)
    printf '%s\n' "$*" >> "$PRESSURE_FIXTURE/cleanup"
    case "$1:${3:-}" in
      rm:wash-pressure-build-*) stall build-remove;;
      rm:*) stall remove;;
      *) stall kill;;
    esac
    if [ -f "$PRESSURE_FIXTURE/start-started" ]; then kill "$(cat "$PRESSURE_FIXTURE/start-started")" 2>/dev/null || :; fi
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
        self.signal_kind(child, event, "-TERM")
    }

    fn signal_kind(&self, child: &mut ChildGuard, event: &str, signal: &str) -> Result<()> {
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
                .args([signal, &child.0.id().to_string()])
                .status()?
                .success(),
            "signal failed"
        );
        self.finished(child)
    }

    fn removed(&self, kind: &str) -> Result<bool> {
        let name = fs::read_to_string(self.path().join(format!("{kind}-name")))?;
        Ok(fs::read_to_string(self.path().join("cleanup"))?
            .lines()
            .any(|line| line == format!("rm --force {}", name.trim())))
    }

    fn spawn(&self, command: &mut Command) -> Result<ChildGuard> {
        Ok(ChildGuard(
            command
                .stdout(Stdio::null())
                .stderr(fs::File::create(self.path().join("driver.log"))?)
                .spawn()?,
        ))
    }

    fn finished(&self, child: &mut ChildGuard) -> Result<()> {
        wait_until(
            || Ok(child.0.try_wait()?.is_some()),
            "driver did not exit before the test deadline",
        )
        .with_context(|| fs::read_to_string(self.path().join("driver.log")).unwrap_or_default())
    }

    fn no_stalled_processes(&self) -> Result<()> {
        for pid in fs::read_to_string(self.path().join("hung-pids"))?.lines() {
            ensure!(
                !Command::new("kill")
                    .args(["-0", pid])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()?
                    .success(),
                "stalled Docker process {pid} survived"
            );
        }
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(pids) = fs::read_to_string(self.path().join("hung-pids")) {
            for pid in pids.lines() {
                let _ = Command::new("kill")
                    .args(["-KILL", pid])
                    .stderr(Stdio::null())
                    .status();
            }
        }
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
    assert!(fixture.removed("case")?);
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
        assert!(fixture.removed("case")?);
    }
    Ok(())
}

#[test]
fn malformed_checkpoint_still_removes_the_container() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    fs::write(fixture.path().join("input.json"), "{broken")?;
    let output = fixture.command(Shape::Sparse, "growth")?.output()?;
    assert!(!output.status.success());
    assert!(fixture.removed("case")?);
    Ok(())
}

#[test]
fn timeout_fails_even_with_an_oom_flag_and_complete_checkpoint() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    fixture.exit(137, true)?;
    let output = fixture
        .command(Shape::Sparse, "growth")?
        .args(["--timeout", "1"])
        .env("HANG_COMMANDS", "start")
        .output()?;
    assert!(!output.status.success());
    assert!(fixture.report()?.stop.contains("timed out"));
    let cleanup = fs::read_to_string(fixture.path().join("cleanup"))?;
    assert!(cleanup.lines().any(|line| line.starts_with("kill ")));
    assert!(fixture.removed("case")?);
    Ok(())
}

#[test]
fn sigterm_during_measurement_removes_the_container() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    let mut child = ChildGuard(
        fixture
            .command(Shape::Sparse, "growth")?
            .env("HANG_COMMANDS", "start")
            .stdout(Stdio::null())
            .stderr(fs::File::create(fixture.path().join("driver.log"))?)
            .spawn()?,
    );
    fixture.signal(&mut child, "start-started")?;
    assert!(fixture.removed("case")?);
    Ok(())
}

#[test]
fn sigterm_during_build_stops_the_build_and_cleans_up() -> Result<()> {
    for omit_cid in ["0", "1"] {
        let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
        let mut child = fixture.spawn(
            fixture
                .command(Shape::Sparse, "growth")?
                .env("HANG_COMMANDS", "build")
                .env("OMIT_BUILD_CID", omit_cid),
        )?;
        fixture.signal(&mut child, "build-started")?;
        assert!(fixture.removed("build")?);
        fixture.no_stalled_processes()?;
    }
    Ok(())
}

#[test]
fn sigint_during_creation_cleans_up_without_a_container_id() -> Result<()> {
    let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
    let mut child = fixture.spawn(
        fixture
            .command(Shape::Sparse, "growth")?
            .env("HANG_COMMANDS", "create"),
    )?;
    fixture.signal_kind(&mut child, "create-started", "-INT")?;
    assert!(!child.0.wait()?.success());
    assert!(fixture.removed("case")?);
    fixture.no_stalled_processes()?;
    Ok(())
}

#[test]
fn sigterm_during_docker_control_commands_cleans_up() -> Result<()> {
    for event in ["image", "create", "inspect", "kill"] {
        let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
        let mut command = fixture.command(Shape::Sparse, "growth")?;
        command.env(
            "HANG_COMMANDS",
            if event == "kill" { "start kill" } else { event },
        );
        if event == "kill" {
            command.args(["--timeout", "1"]);
        }
        let mut child = fixture.spawn(&mut command)?;
        fixture.signal(&mut child, &format!("{event}-started"))?;
        assert!(!child.0.wait()?.success(), "{event} ignored cancellation");
        assert!(
            fs::read_to_string(fixture.path().join("driver.log"))?
                .contains("pressure run interrupted")
        );
        assert!(fixture.removed(if event == "image" { "build" } else { "case" })?);
        fixture.no_stalled_processes()?;
    }
    Ok(())
}

#[test]
fn stalled_docker_control_commands_time_out_and_clean_up() -> Result<()> {
    for event in ["image", "create", "inspect", "kill"] {
        let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
        let mut command = fixture.command(Shape::Sparse, "growth")?;
        command.args(["--docker-timeout", "1"]).env(
            "HANG_COMMANDS",
            if event == "kill" { "start kill" } else { event },
        );
        if event == "kill" {
            command.args(["--timeout", "1"]);
        }
        let mut child = fixture.spawn(&mut command)?;
        fixture.finished(&mut child)?;
        assert!(!child.0.wait()?.success(), "{event} ignored its deadline");
        let name = if event == "image" {
            "image-inspect"
        } else {
            event
        };
        assert!(
            fs::read_to_string(fixture.path().join("driver.log"))?
                .contains(&format!("{name} timed out"))
        );
        assert!(fixture.removed(if event == "image" { "build" } else { "case" })?);
        fixture.no_stalled_processes()?;
    }
    Ok(())
}

#[test]
fn sigterm_during_stalled_removal_is_bounded() -> Result<()> {
    for event in ["build-remove", "remove"] {
        let fixture = Fixture::new(Shape::Sparse, "growth", false)?;
        let mut command = fixture.command(Shape::Sparse, "growth")?;
        command.env("HANG_COMMANDS", event);
        if event == "build-remove" {
            command.args(["--docker-timeout", "1"]);
        }
        let mut child = fixture.spawn(&mut command)?;
        fixture.signal(&mut child, &format!("{event}-started"))?;
        assert!(!child.0.wait()?.success());
        let log = fs::read_to_string(fixture.path().join("driver.log"))?;
        assert!(log.contains("cleanup timed out"));
        assert!(log.contains("pressure run interrupted"));
        assert!(fixture.removed(if event == "build-remove" {
            "build"
        } else {
            "case"
        })?);
        fixture.no_stalled_processes()?;
    }
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
