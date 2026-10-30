// Report provenance uses uname and nproc/getconf.
#![cfg(unix)]

use std::fs::{self, FileTimes};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

struct Repository(tempfile::TempDir);

impl Repository {
    fn new() -> Result<Self> {
        let repo = Self(tempfile::tempdir()?);
        repo.git(&["init", "--quiet", "--template="])?;
        repo.git(&["checkout", "--quiet", "-b", "bench-fixture"])?;
        repo.git(&["commit", "--quiet", "--allow-empty", "-s", "-m", "fixture"])?;
        Ok(repo)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn target(&self) -> PathBuf {
        self.path().join("target")
    }

    fn isolate(&self, command: &mut Command) {
        command
            .current_dir(self.path())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.path().join("no-global-config"));
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        let mut command = Command::new("git");
        self.isolate(&mut command);
        command
            .args([
                "-c",
                "user.name=Benchmark Test",
                "-c",
                "user.email=bench@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .arg("-c")
            .arg(format!(
                "core.hooksPath={}",
                self.path().join("no-hooks").display()
            ))
            .args(args);
        success(&mut command)
    }

    fn command(&self, subcommand: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bench-tools"));
        self.isolate(&mut command);
        command
            .arg(subcommand)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("WASMCLOUD_BENCH_REF")
            .env("GITHUB_REF_NAME", "workflow-dispatch-ref")
            .env("GITHUB_RUN_ID", "fixture-run")
            .env("GITHUB_RUN_ATTEMPT", "3");
        command
    }

    fn marker(&self, bench: &str) -> Result<PathBuf> {
        let path = self.target().join(format!(".bench-start-{bench}"));
        write(&path, [])?;
        modified(&path, 1_000_000)?;
        Ok(path)
    }
}

fn success(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

fn failure(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    ensure!(!output.status.success(), "command unexpectedly succeeded");
    Ok(String::from_utf8(output.stderr)?)
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    fs::create_dir_all(path.parent().context("missing parent directory")?)?;
    fs::write(path, bytes)?;
    Ok(())
}

fn modified(path: &Path, seconds: u64) -> Result<()> {
    fs::File::options().write(true).open(path)?.set_times(
        FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
    )?;
    Ok(())
}

fn criterion(
    root: &Path,
    group: &str,
    param: &str,
    mean: f64,
    throughput: Value,
) -> Result<PathBuf> {
    let estimate = |value| json!({"point_estimate": value, "confidence_interval": {"lower_bound": value * 0.9, "upper_bound": value * 1.1}});
    let path = root
        .join("criterion")
        .join(group)
        .join(param)
        .join("new/estimates.json");
    write(
        &path,
        serde_json::to_vec(
            &json!({"mean": estimate(mean), "median": estimate(mean * 0.8), "std_dev": estimate(mean * 0.1)}),
        )?,
    )?;
    write(
        &path.with_file_name("benchmark.json"),
        serde_json::to_vec(&json!({"throughput": throughput}))?,
    )?;
    modified(&path, 1_000_010)?;
    Ok(path)
}

fn callgrind(root: &Path, group: &str, param: &str, file: &str, ir: u64) -> Result<()> {
    write(
        &root
            .join("gungraun/pkg/bench")
            .join(group)
            .join(param)
            .join(file),
        format!("events: Dr Ir Dw\nsummary: 5 {ir} 7\n"),
    )
}

fn rows(text: &str) -> Result<Vec<Value>> {
    text.lines()
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

fn provenance(row: &Value, repo: &Repository, expected_ref: &str) -> Result<()> {
    assert_eq!(row["sha"], repo.git(&["rev-parse", "HEAD"])?.trim());
    assert_eq!(
        row["short_sha"],
        repo.git(&["rev-parse", "--short=12", "HEAD"])?.trim()
    );
    assert_eq!(row["ref"], expected_ref);
    assert_eq!(row["run_id"], "fixture-run");
    assert_eq!(row["run_attempt"], "3");
    assert!(row["host"].as_str().is_some_and(|v| !v.is_empty()));
    assert!(row["kernel"].as_str().is_some_and(|v| !v.is_empty()));
    assert!(row["cpus_online"].as_u64().is_some_and(|v| v > 0));
    assert!(
        row["timestamp"]
            .as_str()
            .is_some_and(|v| v.len() == 20 && v.ends_with('Z'))
    );
    Ok(())
}

#[test]
fn criterion_exports_keep_estimates_and_summaries_convert_throughput_units() -> Result<()> {
    let repo = Repository::new()?;
    repo.marker("http_invoke")?;
    criterion(
        &repo.target(),
        "batch",
        "p3",
        1e6,
        json!({"Elements": 1000}),
    )?;
    criterion(
        &repo.target(),
        "bytes",
        "p2",
        1e9,
        json!({"BytesDecimal": 2048}),
    )?;
    criterion(&repo.target(), "duration", "p2", 1500.0, Value::Null)?;
    let stale = criterion(&repo.target(), "stale", "ignored", 1.0, Value::Null)?;
    write(&stale, b"stale malformed output")?;
    modified(&stale, 999_990)?;
    let data = rows(&success(
        repo.command("jsonl").args(["--bench", "http_invoke"]),
    )?)?;
    assert_eq!(data.len(), 3);
    for row in &data {
        provenance(row, &repo, "bench-fixture")?;
        assert_eq!(row["bench"], "http_invoke");
        assert_eq!(row["metric"], "mean_ns");
        assert_eq!(row["value"], row["mean_ns"]);
        let mean = row["mean_ns"].as_f64().context("missing mean")?;
        assert_eq!(row["median_ns"], mean * 0.8);
        assert_eq!(row["std_dev_ns"], mean * 0.1);
        assert_eq!(row["ci_low_ns"], mean * 0.9);
        assert_eq!(row["ci_high_ns"], mean * 1.1);
    }
    assert!(
        data.iter()
            .any(|row| row["group"] == "batch" && row["throughput"] == json!({"Elements": 1000}))
    );
    assert!(
        data.iter().any(
            |row| row["group"] == "bytes" && row["throughput"] == json!({"BytesDecimal": 2048})
        )
    );
    let summary = success(repo.command("summary").args([
        "--bench",
        "http_invoke",
        "--run-id",
        "artifact-run",
    ]))?;
    assert!(summary.contains("| batch | p3 | req/s | 1.00 Mreq/s |"));
    assert!(summary.contains("| bytes | p2 | B/s | 2.05 KB/s |"));
    assert!(summary.contains("| duration | p2 | time | 1.50 µs |"));
    assert!(summary.contains("bench-http_invoke-artifact-run"));
    assert!(summary.contains("`bench-fixture`"));
    assert!(!summary.contains("stale"));
    Ok(())
}

#[test]
fn missing_markers_suppress_stale_data_and_explicit_markers_override_defaults() -> Result<()> {
    let repo = Repository::new()?;
    criterion(&repo.target(), "group", "param", 1000.0, Value::Null)?;
    let mut command = repo.command("jsonl");
    let output: Output = command.args(["--bench", "wall"]).output()?;
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)?.contains("no marker at"));
    let empty = success(repo.command("summary").args(["--bench", "wall"]))?;
    assert!(empty.contains("no criterion output"));
    let marker = repo.marker("different-bench")?;
    let text = success(
        repo.command("jsonl")
            .args(["--bench", "wall", "--marker"])
            .arg(marker),
    )?;
    assert_eq!(rows(&text)?.len(), 1);
    Ok(())
}

#[test]
fn gungraun_exports_sum_threads_and_render_instruction_counts() -> Result<()> {
    let repo = Repository::new()?;
    callgrind(
        &repo.target(),
        "invoke",
        "p2",
        "callgrind.p2.t1.p1.out",
        1_200_000,
    )?;
    callgrind(&repo.target(), "invoke", "p2", "callgrind.p2.t2.p1.out", 0)?;
    callgrind(
        &repo.target(),
        "invoke",
        "p2",
        "callgrind.p2.t1.p2.out",
        34_567,
    )?;
    callgrind(&repo.target(), "invoke", "p3", "callgrind.out", 900)?;
    let text = success(
        repo.command("jsonl")
            .args(["--bench", "gungraun_plugin", "--target-dir"])
            .arg(repo.target()),
    )?;
    let data = rows(&text)?;
    assert_eq!(data.len(), 2);
    for row in &data {
        provenance(row, &repo, "bench-fixture")?;
        assert_eq!(row["metric"], "Ir");
        assert!(row.get("mean_ns").is_none());
        assert!(row.get("throughput").is_none());
    }
    assert!(
        data.iter()
            .any(|r| r["param"] == "p2" && r["value"] == 1_234_567.0)
    );
    assert!(
        data.iter()
            .any(|r| r["param"] == "p3" && r["value"] == 900.0)
    );
    let summary = success(
        repo.command("summary")
            .args(["--bench", "gungraun_plugin", "--target-dir"])
            .arg(repo.target()),
    )?;
    assert!(summary.contains("| invoke | p2 | 1,234,567 |"));
    assert!(summary.contains("bench-gungraun_plugin-fixture-run"));
    fs::remove_dir_all(repo.target().join("gungraun"))?;
    assert!(success(repo.command("jsonl").args(["--bench", "gungraun_plugin"]))?.is_empty());
    let empty = success(repo.command("summary").args(["--bench", "gungraun_plugin"]))?;
    assert!(empty.contains("Instruction-count bench"));
    assert!(!empty.contains("| group | param |"));
    Ok(())
}

#[test]
fn density_exports_publish_clean_measurements_and_preserve_sparse_reports() -> Result<()> {
    let repo = Repository::new()?;
    let step = |workloads, clean, p99| {
        json!({
            "workloads":workloads,"started":10,"starts_per_sec":100.0,"start_cpu_secs":0.01,
            "start_p99_ms":2.0,"request_p99_ms":p99,"spin_down_ms":4.0,"clean":clean
        })
    };
    let reports = json!([
        {"bench":"scale_density","label":"warm","stop":"memory","limits":{"memory":400,"pool":200,"guest":null},
         "marginal_bytes_per_workload":1024,"baseline_footprint_bytes":1048576,
         "steps":[step(10,true,1.0),step(20,true,3.0),step(30,false,999.0)]},
        {"bench":"scale_density","label":"scale-to-zero@2cpu","stop":"unmeasured"}
    ]);
    write(
        &repo.target().join("density/scale_density.json"),
        serde_json::to_vec(&reports)?,
    )?;
    let data = rows(&success(
        repo.command("jsonl").args(["--bench", "scale_density"]),
    )?)?;
    assert_eq!(data.len(), 10);
    for row in &data {
        provenance(row, &repo, "bench-fixture")?;
        assert_eq!(row["group"], "density");
        assert!(row["unit"].is_string());
        assert!(row["better"] == "higher" || row["better"] == "lower");
    }
    assert!(
        data.iter().any(|r| r["param"] == "warm"
            && r["metric"] == "served_workloads"
            && r["value"] == 20.0)
    );
    assert!(
        data.iter()
            .any(|r| r["metric"] == "binding_limit" && r["value"] == 200.0)
    );
    assert!(
        data.iter().any(|r| r["metric"] == "request_p99_ms"
            && r["value"] == 3.0
            && r["better"] == "lower")
    );
    assert_eq!(
        data.iter()
            .filter(|r| r["param"] == "scale-to-zero@2cpu")
            .count(),
        1
    );
    let summary = success(repo.command("summary").args(["--bench", "scale_density"]))?;
    assert!(summary.contains("| warm | served_workloads | 20.00 | workloads | higher | memory |"));
    assert!(summary.contains("| warm | request_p99_ms | 3.00 | ms | lower | memory |"));
    assert!(summary.contains(
        "| scale-to-zero@2cpu | served_workloads | 0.00 | workloads | higher | unmeasured |"
    ));
    assert!(!summary.contains("999"));
    Ok(())
}

fn delta(repo: &Repository, bench: &str, compare: &Path) -> Result<String> {
    success(
        repo.command("delta")
            .args(["--bench", bench, "--compare-dir"])
            .arg(compare)
            .args([
                "--ref-a",
                "baseline",
                "--ref-b",
                "candidate",
                "--short-a",
                "aaaa",
                "--short-b",
                "bbbb",
                "--iters",
                "3",
            ]),
    )
}

#[test]
fn criterion_delta_uses_iteration_medians_and_a_sorted_union_of_measurements() -> Result<()> {
    let repo = Repository::new()?;
    let compare = repo.path().join("comparison");
    for (side, values) in [("a", [100.0, 300.0, 200.0]), ("b", [150.0, 250.0, 500.0])] {
        for (i, mean) in values.into_iter().enumerate() {
            criterion(
                &compare.join(side).join(format!("iter-{i}")),
                "group",
                "shared",
                mean,
                Value::Null,
            )?;
        }
    }
    criterion(
        &compare.join("a/iter-0"),
        "group",
        "removed",
        10.0,
        Value::Null,
    )?;
    criterion(
        &compare.join("b/iter-0"),
        "group",
        "added",
        20.0,
        Value::Null,
    )?;
    criterion(&compare.join("a/iter-0"), "group", "zero", 0.0, Value::Null)?;
    criterion(&compare.join("b/iter-0"), "group", "zero", 5.0, Value::Null)?;
    criterion(
        &compare.join("a/unrelated"),
        "group",
        "shared",
        9999.0,
        Value::Null,
    )?;
    write(&compare.join("a/not-a-directory"), b"ignored")?;
    let text = delta(&repo, "http_invoke", &compare)?;
    assert!(text.contains("| group | shared | 200 | 250 | 25.00% |"));
    assert!(text.contains("| group | removed | 10 | — | — |"));
    assert!(text.contains("| group | added | — | 20 | — |"));
    assert!(text.contains("| group | zero | 0 | 5 | — |"));
    assert!(text.find("| group | added |") < text.find("| group | removed |"));
    assert_eq!(text, fs::read_to_string(compare.join("delta.md"))?);
    assert!(text.contains("mean time (ns)"));
    assert!(text.contains("`baseline` @ `aaaa`"));
    Ok(())
}

#[test]
fn gungraun_delta_compares_summed_counts_and_handles_missing_sides() -> Result<()> {
    let repo = Repository::new()?;
    let compare = repo.path().join("comparison");
    callgrind(
        &compare.join("a/iter-1"),
        "invoke",
        "p2",
        "callgrind.t1.p1.out",
        100,
    )?;
    callgrind(
        &compare.join("a/iter-1"),
        "invoke",
        "p2",
        "callgrind.t2.p1.out",
        50,
    )?;
    callgrind(
        &compare.join("b/iter-1"),
        "invoke",
        "p2",
        "callgrind.out",
        120,
    )?;
    let text = delta(&repo, "gungraun_new_target", &compare)?;
    assert!(text.contains("Ir (instructions) (instr)"));
    assert!(text.contains("| invoke | p2 | 150 | 120 | -20.00% |"));
    fs::remove_dir_all(compare.join("a"))?;
    assert!(
        delta(&repo, "gungraun_new_target", &compare)?.contains("| invoke | p2 | — | 120 | — |")
    );
    fs::remove_dir_all(compare.join("b"))?;
    let empty = delta(&repo, "gungraun_new_target", &compare)?;
    assert!(empty.contains("no measurement output found"));
    assert_eq!(empty, fs::read_to_string(compare.join("delta.md"))?);
    Ok(())
}

#[test]
fn malformed_inputs_fail_instead_of_publishing_partial_measurements() -> Result<()> {
    let repo = Repository::new()?;
    repo.marker("wall")?;
    let estimates = criterion(&repo.target(), "group", "param", 1.0, Value::Null)?;
    write(&estimates, b"bad estimates")?;
    modified(&estimates, 1_000_010)?;
    for command in ["jsonl", "summary"] {
        assert!(
            failure(repo.command(command).args(["--bench", "wall"]))?.contains("estimates.json")
        );
    }
    write(&repo.target().join("density/density.json"), b"bad density")?;
    for command in ["jsonl", "summary"] {
        assert!(
            failure(repo.command(command).args(["--bench", "density"]))?.contains("density.json")
        );
    }
    let output = repo
        .target()
        .join("gungraun/pkg/bench/group/param/callgrind.out");
    write(&output, "events: Ir\nsummary: invalid\n")?;
    for command in ["jsonl", "summary"] {
        assert!(
            failure(repo.command(command).args(["--bench", "gungraun"]))?.contains("summary[0]")
        );
    }
    let compare = repo.path().join("comparison");
    fs::create_dir_all(compare.join("a"))?;
    write(&compare.join("delta.md"), [])?;
    fs::remove_file(compare.join("delta.md"))?;
    fs::create_dir(compare.join("delta.md"))?;
    assert!(
        failure(
            repo.command("delta")
                .args(["--bench", "wall", "--compare-dir"])
                .arg(&compare)
                .args([
                    "--ref-a",
                    "a",
                    "--ref-b",
                    "b",
                    "--short-a",
                    "a",
                    "--short-b",
                    "b",
                    "--iters",
                    "1"
                ])
        )?
        .contains("delta.md")
    );
    Ok(())
}

#[test]
fn missing_git_provenance_fails_without_emitting_measurements() -> Result<()> {
    let repo = Repository::new()?;
    fs::remove_dir_all(repo.path().join(".git"))?;
    for subcommand in ["jsonl", "summary"] {
        let output = repo
            .command(subcommand)
            .args(["--bench", "wall"])
            .output()?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8(output.stderr)?.contains("git rev-parse HEAD exited"));
    }
    Ok(())
}

#[test]
fn metadata_prefers_explicit_refs_then_exact_tags_branches_and_detached_shas() -> Result<()> {
    let repo = Repository::new()?;
    callgrind(&repo.target(), "group", "param", "callgrind.out", 1)?;
    repo.git(&["tag", "v99.0.0"])?;
    let export = |reference: Option<&str>| -> Result<Vec<Value>> {
        let mut command = repo.command("jsonl");
        command.args(["--bench", "gungraun"]);
        if let Some(value) = reference {
            command.env("WASMCLOUD_BENCH_REF", value);
        }
        rows(&success(&mut command)?)
    };
    for row in export(Some("requested-ref"))? {
        provenance(&row, &repo, "requested-ref")?;
    }
    for row in export(None)? {
        provenance(&row, &repo, "v99.0.0")?;
    }
    repo.git(&["tag", "-d", "v99.0.0"])?;
    for row in export(Some(""))? {
        provenance(&row, &repo, "bench-fixture")?;
    }
    repo.git(&["checkout", "--quiet", "--detach"])?;
    let sha = repo.git(&["rev-parse", "--short=12", "HEAD"])?;
    for row in export(None)? {
        provenance(&row, &repo, sha.trim())?;
    }
    let mut command = repo.command("jsonl");
    command
        .args(["--bench", "gungraun"])
        .env("GITHUB_RUN_ID", "")
        .env_remove("GITHUB_RUN_ATTEMPT");
    for row in rows(&success(&mut command)?)? {
        assert_eq!(row["run_id"], "local");
        assert_eq!(row["run_attempt"], "1");
    }
    let summary = success(
        repo.command("summary")
            .args(["--bench", "gungraun"])
            .env_remove("GITHUB_RUN_ID"),
    )?;
    assert!(summary.contains("bench-gungraun-local"));
    Ok(())
}

#[test]
fn environment_target_directory_and_comparison_arguments_are_supported() -> Result<()> {
    let repo = Repository::new()?;
    let target = repo.path().join("custom-target");
    callgrind(&target, "group", "param", "callgrind.out", 77)?;
    let data = rows(&success(
        repo.command("jsonl")
            .args(["--bench", "gungraun"])
            .env("CARGO_TARGET_DIR", &target),
    )?)?;
    assert!(data.iter().any(|row| row["value"] == 77.0));
    let compare = repo.path().join("comparison");
    callgrind(
        &compare.join("a/iter-1"),
        "group",
        "param",
        "callgrind.out",
        10,
    )?;
    let output = success(
        repo.command("delta")
            .env("WASMCLOUD_BENCH_COMPARE_DIR", &compare)
            .env("WASMCLOUD_BENCH_NAME", "gungraun")
            .env("WASMCLOUD_BENCH_REF_A", "old")
            .env("WASMCLOUD_BENCH_REF_B", "new")
            .env("WASMCLOUD_BENCH_SHORT_A", "aaaa")
            .env("WASMCLOUD_BENCH_SHORT_B", "bbbb")
            .env("WASMCLOUD_BENCH_ITERS", "1"),
    )?;
    assert!(output.contains("| group | param | 10 | — | — |"));
    Ok(())
}
