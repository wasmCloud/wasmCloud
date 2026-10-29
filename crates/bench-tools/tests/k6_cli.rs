use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(profile: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/k6")
        .join(profile)
}

#[test]
fn report_and_jsonl_commands_keep_measurements_and_provenance() -> anyhow::Result<()> {
    for profile in ["constant", "stress"] {
        for markdown in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_bench-tools"));
            command.args(["k6", "report"]).arg(fixture(profile));
            if markdown {
                command.arg("--markdown");
            }
            let output = command.output()?;
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout)?;
            assert!(text.contains("Headline") && text.contains("Verdict"));
            assert!(text.contains("thresholds"));
            if markdown {
                assert!(text.contains("| scenario |"));
            }
        }
        let output = Command::new(env!("CARGO_BIN_EXE_bench-tools"))
            .args(["k6", "jsonl"])
            .arg(fixture(profile))
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout)?;
        let rows = text
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?;
        assert!(!rows.is_empty());
        for row in rows {
            assert_eq!(row["bench"], "k6");
            assert!(row["value"].is_number());
            assert!(row["metric"].as_str().is_some_and(|s| !s.is_empty()));
            assert!(row["sha"].as_str().is_some_and(|s| !s.is_empty()));
            assert!(row["thresholds_passed"].is_boolean());
            assert!(row["generator_saturated"].is_boolean());
        }
    }
    Ok(())
}

#[test]
fn delta_reports_unchanged_runs_and_rejects_incomparable_loads() -> anyhow::Result<()> {
    for markdown in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bench-tools"));
        command
            .args(["k6", "delta"])
            .arg(fixture("constant"))
            .arg(fixture("constant"));
        if markdown {
            command.arg("--markdown");
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8(output.stdout)?.contains("≈ noise"));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_bench-tools"))
        .args(["k6", "delta"])
        .arg(fixture("constant"))
        .arg(fixture("stress"))
        .output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("runs don't compare"));
    Ok(())
}
