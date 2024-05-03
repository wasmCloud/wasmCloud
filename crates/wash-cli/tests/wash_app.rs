use anyhow::{Context, Result};
use tokio::process::Command;
use wash_lib::cli::output::AppValidateOutput;

/// Ensure a simple WADM manifest passes validation
#[tokio::test]
async fn app_validate_simple() -> Result<()> {
    let pass = "./tests/fixtures/wadm/simple.wadm.yaml";
    tokio::fs::try_exists(pass).await?;
    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "app",
            "validate",
            "./tests/fixtures/wadm/manifests/simple.wadm.yaml",
            "--output",
            "json",
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute wash app validate")?;

    let cmd_output: AppValidateOutput =
        serde_json::from_slice(&output.stdout).context("failed to build JSON from output")?;
    assert!(cmd_output.valid, "valid output");
    assert!(cmd_output.errors.is_empty(), "no errors");
    assert!(cmd_output.warnings.is_empty(), "no warnings");

    Ok(())
}
