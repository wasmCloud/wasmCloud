use anyhow::{Context, Result};
use tokio::process::Command;
use wash_lib::cli::output::StartCommandOutput;

mod common;
use common::{TestWashInstance, ECHO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

#[tokio::test]
async fn integration_start_actor_serial() -> Result<()> {
    let _wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "start",
            "actor",
            ECHO_OCI_REF,
            "--output",
            "json",
            "--timeout-ms",
            "20000",
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to start actor")?;

    assert!(output.status.success(), "executed start");

    let cmd_output: StartCommandOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}

#[tokio::test]
async fn integration_start_provider_serial() -> Result<()> {
    let _wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "start",
            "provider",
            PROVIDER_HTTPSERVER_OCI_REF,
            "--output",
            "json",
            "--timeout-ms",
            "20000",
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to start provider")?;

    assert!(output.status.success(), "executed start");

    let cmd_output: StartCommandOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}
