use anyhow::{Context, Result};
use serde::Deserialize;
use serial_test::serial;

mod common;
use common::{wash, TestWashInstance, ECHO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

#[derive(Debug, Deserialize)]
struct StartOutput {
    success: bool,
}

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

    let cmd_output: StartOutput =
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

    let cmd_output: StartOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}
