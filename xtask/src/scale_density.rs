//! `cargo xtask scale-density <preset>`: density, core-sweep and stock presets
//! for wash-runtime's `scale_density` bench. A preset fills in only the
//! settings the caller left unset; `crates/wash-runtime/benches/scale_density.md`
//! describes each one.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest as _, Sha256};

const FIXTURE: &str = "http-handler-p3";

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Preset {
    /// Warm and scale-to-zero ramps, with a dense layout for warm.
    Density,
    /// Both flavours at 1, 2 and 4 cores: pinned on Linux, host runtime
    /// workers elsewhere.
    Cores,
    /// The bench's own defaults.
    Stock,
}

/// One `cargo bench` run and the settings it passes.
#[derive(Debug, PartialEq, Eq)]
struct Run {
    name: &'static str,
    flavour: Option<&'static str>,
    env: Vec<(&'static str, String)>,
}

/// The runs a preset makes; a value from `var` wins over the preset's.
fn runs(preset: Preset, var: impl Fn(&str) -> Option<String>) -> Vec<Run> {
    let tuned = [
        ("WASH_SCALE_MEMORY_LIMIT", "2GiB"),
        ("WASMTIME_POOLING_TABLE_KEEP_RESIDENT", "65536"),
        ("WASH_SCALE_START_CONCURRENCY", "64"),
        ("WASH_SCALE_STEP", "10000"),
        ("WASH_SCALE_SAMPLE", "20000"),
        ("WASH_SCALE_MAX_MEMORIES_CAP", "300000"),
    ];
    // This layout fits the HTTP fixture; it is not a production default.
    let dense = [
        ("WASH_SCALE_GUEST_MEMORY_MODE", "growth"),
        ("WASH_SCALE_MEMORY_RESERVATION", "2MiB"),
        ("WASH_SCALE_MEMORY_GUARD", "0"),
    ];
    let with = |name, flavour, extra: &[&[(&'static str, &str)]]| Run {
        name,
        flavour: Some(flavour),
        env: std::iter::once(&tuned[..])
            .chain(extra.iter().copied())
            .flatten()
            .map(|&(key, default)| (key, var(key).unwrap_or_else(|| default.to_string())))
            .collect(),
    };
    match preset {
        Preset::Density => {
            let secs = [("WASH_SCALE_MAX_SECS", "1800")];
            vec![
                with("density-warm", "warm", &[&secs, &dense]),
                with("density-s2z", "scale-to-zero", &[&secs]),
            ]
        }
        Preset::Cores => {
            let cpus = [("WASH_SCALE_CPUS", "1,2,4")];
            vec![
                with(
                    "cores-warm",
                    "warm",
                    &[&cpus, &[("WASH_SCALE_MAX_COMPONENTS", "50000")], &dense],
                ),
                with(
                    "cores-s2z",
                    "scale-to-zero",
                    &[&cpus, &[("WASH_SCALE_MAX_COMPONENTS", "100000")]],
                ),
            ]
        }
        Preset::Stock => vec![Run {
            name: "stock",
            flavour: None,
            env: Vec::new(),
        }],
    }
}

pub(crate) fn run(workspace: &Path, preset: Preset, out: Option<PathBuf>) -> Result<()> {
    let var = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let out = out.unwrap_or_else(|| crate::target_dir(workspace).join("scale-density"));
    run_presets(workspace, preset, &out, var, || Command::new(&cargo))
}

/// [`run`] with the environment and the `cargo` it runs supplied.
fn run_presets(
    workspace: &Path,
    preset: Preset,
    out: &Path,
    var: impl Fn(&str) -> Option<String>,
    cargo: impl Fn() -> Command,
) -> Result<()> {
    // Every preset runs the warm flavour, which needs the P3 fixture.
    if let Some(fixture) = var("WASH_SCALE_FIXTURE")
        && fixture.trim() != "p3"
    {
        bail!("the presets run the P3 fixture; unset WASH_SCALE_FIXTURE={fixture}");
    }
    let runs = runs(preset, var);
    // Read before this task writes anything, so its own output never counts.
    let source = source_revision(workspace);

    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let out = fs::canonicalize(out)?;
    let fixture = workspace.join("crates/wash-runtime/tests/wasm/http_handler_p3.wasm");
    if !fixture.exists() {
        crate::build_fixtures(workspace, &[FIXTURE.to_string()])?;
    }
    let digest = Sha256::digest(
        fs::read(&fixture).with_context(|| format!("reading {}", fixture.display()))?,
    );
    let provenance = format!(
        "git: {source}\nfixture: {digest:x}\nhost: {}\n",
        host_description()
    );

    // Built in the open, so a cold build's progress shows; the runs are quiet.
    let status = cargo()
        .args(["bench", "--locked", "-p", "wash-runtime"])
        .args(["--bench", "scale_density", "--no-run"])
        .current_dir(workspace)
        .status()
        .context("building the scale_density bench")?;
    if !status.success() {
        bail!("building the scale_density bench failed: {status}");
    }
    let mut failed = Vec::new();
    for run in &runs {
        let mut header = format!(
            "=== {} ({}) ===\n",
            run.name,
            run.flavour.unwrap_or("all flavours")
        );
        if !run.env.is_empty() {
            let env: Vec<String> = run.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            writeln!(header, "env: {}", env.join(" "))?;
        }
        header.push_str(&provenance);
        print!("{header}");
        let log_path = out.join(format!("{}.log", run.name));
        let mut log =
            File::create(&log_path).with_context(|| format!("creating {}", log_path.display()))?;
        log.write_all(header.as_bytes())?;

        let mut command = cargo();
        command
            .args(["bench", "--locked", "-q", "-p", "wash-runtime"])
            .args(["--bench", "scale_density", "--"])
            .args(run.flavour)
            .envs(run.env.iter().map(|(key, value)| (*key, value.as_str())))
            .env(
                "WASH_SCALE_REPORT_JSON",
                out.join(format!("{}.json", run.name)),
            )
            .current_dir(workspace);
        // A failed flavour does not skip the next; the task still fails.
        if let Err(error) = crate::coverage::execute(command, Some(log), true) {
            eprintln!("{} failed: {error:#}", run.name);
            failed.push(run.name);
        }
    }
    match failed.as_slice() {
        [] => Ok(()),
        names => bail!("{} failed; logs are in {}", names.join(", "), out.display()),
    }
}

/// The commit, and whether the tree held uncommitted or untracked changes.
fn source_revision(workspace: &Path) -> String {
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let Some(commit) = git(&["rev-parse", "HEAD"]) else {
        return "unknown".to_string();
    };
    match git(&["status", "--porcelain"]) {
        Some(status) if status.is_empty() => commit,
        Some(_) => format!("{commit} with uncommitted changes"),
        None => format!("{commit}, status unknown"),
    }
}

/// OS and its version, architecture, CPU model where the platform reports
/// one, and core count.
fn host_description() -> String {
    let cores = std::thread::available_parallelism().map_or(0, usize::from);
    let model = cpu_model().unwrap_or_else(|| "unknown CPU".to_string());
    let os = os_version().unwrap_or_else(|| std::env::consts::OS.to_string());
    format!("{os} {}, {model}, {cores} cores", std::env::consts::ARCH)
}

/// The OS release and kernel, such as `macOS 26.1 (Darwin 25.1.0)` or
/// `Ubuntu 24.04.1 LTS (Linux 6.8.0-49-generic)`.
fn os_version() -> Option<String> {
    let stdout = |program: &str, args: &[&str]| {
        let output = Command::new(program).args(args).output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (output.status.success() && !text.is_empty()).then_some(text)
    };
    let kernel = stdout("uname", &["-sr"]);
    let release = if cfg!(target_os = "macos") {
        stdout("sw_vers", &["-productVersion"]).map(|version| format!("macOS {version}"))
    } else if cfg!(target_os = "linux") {
        fs::read_to_string("/etc/os-release")
            .ok()?
            .lines()
            .find_map(|line| {
                Some(
                    line.strip_prefix("PRETTY_NAME=")?
                        .trim_matches('"')
                        .to_string(),
                )
            })
    } else {
        None
    };
    match (release, kernel) {
        (Some(release), Some(kernel)) => Some(format!("{release} ({kernel})")),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}

fn cpu_model() -> Option<String> {
    let model = if cfg!(target_os = "macos") {
        let output = Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else if cfg!(target_os = "linux") {
        fs::read_to_string("/proc/cpuinfo")
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("model name")?.split_once(':'))?
            .1
            .to_string()
    } else {
        std::env::var("PROCESSOR_IDENTIFIER").ok()?
    };
    let model = model.trim();
    (!model.is_empty()).then(|| model.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    fn value<'a>(run: &'a Run, key: &str) -> Option<&'a str> {
        run.env
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn density_gives_warm_alone_the_dense_layout() {
        let runs = runs(Preset::Density, none);
        let names: Vec<_> = runs.iter().map(|r| (r.name, r.flavour)).collect();
        assert_eq!(
            names,
            [
                ("density-warm", Some("warm")),
                ("density-s2z", Some("scale-to-zero"))
            ]
        );
        let [warm, s2z] = &runs[..] else {
            unreachable!()
        };
        assert_eq!(value(warm, "WASH_SCALE_MEMORY_RESERVATION"), Some("2MiB"));
        assert_eq!(value(warm, "WASH_SCALE_GUEST_MEMORY_MODE"), Some("growth"));
        assert_eq!(value(s2z, "WASH_SCALE_MEMORY_RESERVATION"), None);
        for run in &runs {
            assert_eq!(value(run, "WASH_SCALE_MAX_SECS"), Some("1800"));
            assert_eq!(value(run, "WASH_SCALE_MAX_MEMORIES_CAP"), Some("300000"));
            assert_eq!(value(run, "WASH_SCALE_MEMORY_LIMIT"), Some("2GiB"));
        }
    }

    #[test]
    fn cores_sweeps_workers_with_a_cap_per_flavour() {
        let runs = runs(Preset::Cores, none);
        let caps: Vec<_> = runs
            .iter()
            .map(|r| (r.name, value(r, "WASH_SCALE_MAX_COMPONENTS")))
            .collect();
        assert_eq!(
            caps,
            [("cores-warm", Some("50000")), ("cores-s2z", Some("100000"))]
        );
        assert!(
            runs.iter()
                .all(|r| value(r, "WASH_SCALE_CPUS") == Some("1,2,4"))
        );
        assert_eq!(value(&runs[0], "WASH_SCALE_MEMORY_GUARD"), Some("0"));
        assert_eq!(value(&runs[1], "WASH_SCALE_MEMORY_GUARD"), None);
    }

    #[test]
    fn a_caller_setting_wins_over_the_preset() {
        let var = |key: &str| match key {
            "WASH_SCALE_GUEST_MEMORY_MODE" => Some("enforce".to_string()),
            "WASH_SCALE_MEMORY_RESERVATION" => Some("64MiB".to_string()),
            "WASH_SCALE_CPUS" => Some("1,8".to_string()),
            _ => None,
        };
        let runs = runs(Preset::Cores, var);
        assert_eq!(
            value(&runs[0], "WASH_SCALE_GUEST_MEMORY_MODE"),
            Some("enforce")
        );
        assert_eq!(
            value(&runs[0], "WASH_SCALE_MEMORY_RESERVATION"),
            Some("64MiB")
        );
        assert_eq!(value(&runs[1], "WASH_SCALE_CPUS"), Some("1,8"));
    }

    /// A workspace holding only the fixture, and a stub `cargo` that records
    /// each call in `calls` and fails where `fail_on` matches its arguments.
    #[cfg(unix)]
    fn stub_workspace(fail_on: &str) -> (tempfile::TempDir, impl Fn() -> Command) {
        let dir = tempfile::tempdir().unwrap();
        let wasm = dir.path().join("crates/wash-runtime/tests/wasm");
        fs::create_dir_all(&wasm).unwrap();
        fs::write(wasm.join("http_handler_p3.wasm"), b"fixture").unwrap();
        let script = dir.path().join("cargo.sh");
        fs::write(
            &script,
            format!(
                "echo \"$* report=$WASH_SCALE_REPORT_JSON\" >> calls\n\
                 echo \"bench output for $*\"\n\
                 case \"$*\" in *\"{fail_on}\"*) exit 1;; esac\n"
            ),
        )
        .unwrap();
        let cargo = move || {
            let mut command = Command::new("sh");
            command.arg(&script);
            command
        };
        (dir, cargo)
    }

    #[cfg(unix)]
    #[test]
    fn another_fixture_is_refused_before_anything_is_written() {
        let (dir, cargo) = stub_workspace("never");
        let out = dir.path().join("out");
        let var = |key: &str| (key == "WASH_SCALE_FIXTURE").then(|| "p2".to_string());
        let error = run_presets(dir.path(), Preset::Density, &out, var, cargo).unwrap_err();
        assert!(
            error.to_string().contains("WASH_SCALE_FIXTURE=p2"),
            "{error}"
        );
        assert!(!out.exists());
        assert!(!dir.path().join("calls").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_run_does_not_skip_the_next_but_the_task_fails() {
        let (dir, cargo) = stub_workspace("-- warm");
        let out = dir.path().join("out");
        let error = run_presets(dir.path(), Preset::Density, &out, none, cargo).unwrap_err();
        assert!(error.to_string().contains("density-warm failed"), "{error}");
        assert!(!error.to_string().contains("density-s2z"), "{error}");

        let out = fs::canonicalize(&out).unwrap();
        let calls = fs::read_to_string(dir.path().join("calls")).unwrap();
        let calls: Vec<_> = calls.lines().collect();
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert!(calls[0].contains("--no-run"), "{calls:?}");
        assert!(calls[1].ends_with(&format!(
            "-- warm report={}",
            out.join("density-warm.json").display()
        )));
        assert!(calls[2].contains("-- scale-to-zero report="), "{calls:?}");

        let log = fs::read_to_string(out.join("density-s2z.log")).unwrap();
        let digest = format!("{:x}", Sha256::digest(b"fixture"));
        assert!(
            log.starts_with("=== density-s2z (scale-to-zero) ===\nenv: "),
            "{log}"
        );
        assert!(log.contains("WASH_SCALE_MAX_SECS=1800"), "{log}");
        assert!(log.contains("git: unknown\n"), "{log}");
        assert!(log.contains(&format!("fixture: {digest}\n")), "{log}");
        assert!(log.contains("\nhost: "), "{log}");
        assert!(log.contains("bench output for bench"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_build_runs_nothing() {
        let (dir, cargo) = stub_workspace("--no-run");
        let out = dir.path().join("out");
        let error = run_presets(dir.path(), Preset::Cores, &out, none, cargo).unwrap_err();
        assert!(error.to_string().contains("building"), "{error}");
        let calls = fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(calls.lines().count(), 1, "{calls}");
        assert!(!out.join("cores-warm.log").exists());
    }

    #[test]
    fn the_host_line_names_the_os_version() {
        let host = host_description();
        assert!(host.contains(std::env::consts::ARCH), "{host}");
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(host.contains(" ("), "no kernel in {host}");
        }
    }

    #[test]
    fn stock_adds_nothing() {
        assert_eq!(
            runs(Preset::Stock, none),
            [Run {
                name: "stock",
                flavour: None,
                env: Vec::new(),
            }]
        );
    }
}
