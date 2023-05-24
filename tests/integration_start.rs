use anyhow::{Context, Result};
use serial_test::serial;
use wash_lib::cli::output::StartCommandJsonOutput;

mod common;
use common::{wash, TestWashInstance, ECHO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

#[tokio::test]
#[serial]
async fn integration_start_actor() -> Result<()> {
    let _wash_instance = TestWashInstance::create().await?;

    let output = wash()
        .args([
            "start",
            "actor",
            ECHO_OCI_REF,
            "--output",
            "json",
            "--timeout-ms",
            "20000",
        ])
        .output()
        .context("failed to start actor")?;

    assert!(output.status.success(), "executed start");

    let cmd_output: StartCommandJsonOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_start_provider() -> Result<()> {
    let _wash_instance = TestWashInstance::create().await?;

    let output = wash()
        .args([
            "start",
            "provider",
            PROVIDER_HTTPSERVER_OCI_REF,
            "--output",
            "json",
            "--timeout-ms",
            "20000",
        ])
        .output()
        .context("failed to start provider")?;

    assert!(output.status.success(), "executed start");

    let cmd_output: StartCommandJsonOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}
