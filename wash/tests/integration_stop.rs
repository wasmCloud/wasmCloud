use anyhow::{Context, Result};
use serial_test::serial;

mod common;
use common::{wash, TestWashInstance, ECHO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};
use wash_lib::cli::output::{StartCommandJsonOutput, StopCommandJsonOutput};

#[tokio::test]
#[serial]
async fn integration_stop_actor() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

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

    let StartCommandJsonOutput {
        actor_id,
        actor_ref,
        host_id,
        success,
        ..
    } = serde_json::from_slice(&output.stdout).context("failed to parse start output")?;
    assert!(success, "start command returned success");
    let actor_id = actor_id.expect("missing actor_id from start command output");
    let actor_ref = actor_ref.expect("missing actor_ref from start command output");
    assert_eq!(actor_ref, ECHO_OCI_REF, "actor ref matches");
    let host_id = host_id.expect("missing host_id from start command output");
    assert_eq!(host_id, wash_instance.host_id, "host_id matches");

    // Stop the actor
    let output = wash()
        .args(["stop", "actor", &host_id, &actor_id, "--output", "json"])
        .output()
        .context("failed to stop actor")?;

    assert!(output.status.success(), "executed stop");

    let cmd_output: StopCommandJsonOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse stop output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_stop_provider() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

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
        .context("failed to start actor")?;
    assert!(output.status.success(), "executed start");

    let StartCommandJsonOutput {
        provider_id,
        provider_ref,
        host_id,
        link_name,
        contract_id,
        success,
        ..
    } = serde_json::from_slice(&output.stdout).context("failed to parse start output")?;
    assert!(success, "start command returned success");
    let provider_ref = provider_ref.expect("missing provider_ref from start command output");
    assert_eq!(
        provider_ref, PROVIDER_HTTPSERVER_OCI_REF,
        "provider ref matches"
    );
    let provider_id = provider_id.expect("missing provider_id from start command output");
    let host_id = host_id.expect("missing host_id from start command output");
    assert_eq!(host_id, wash_instance.host_id, "host_id matches");
    let link_name = link_name.expect("missing link_name from start command output");
    let contract_id = contract_id.expect("missing contract_id from start command output");

    let output = wash()
        .args([
            "stop",
            "provider",
            &host_id,
            &provider_id,
            &link_name,
            &contract_id,
            "--output",
            "json",
        ])
        .output()
        .context("failed to stop provider")?;

    assert!(output.status.success(), "executed stop");

    let cmd_output: StopCommandJsonOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse stop output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_stop_host() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = wash()
        .args(["stop", "host", &wash_instance.host_id, "--output", "json"])
        .output()
        .context("failed to stop provider")?;

    assert!(output.status.success(), "executed stop");

    let cmd_output: StopCommandJsonOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}
