mod common;

use common::{TestWashInstance, ECHO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

use anyhow::{Context, Result};
use serial_test::serial;
use tokio::process::Command;
use wash_lib::cli::output::{StartCommandOutput, StopCommandOutput};

#[tokio::test]
#[serial]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
async fn integration_stop_actor_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "start",
            "actor",
            ECHO_OCI_REF,
            "--output",
            "json",
            "--timeout-ms",
            "20000",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .output()
        .await
        .context("failed to start actor")?;
    assert!(output.status.success(), "executed start");

    let StartCommandOutput {
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
    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "stop",
            "actor",
            &host_id,
            &actor_id,
            "--output",
            "json",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .output()
        .await
        .context("failed to stop actor")?;

    assert!(output.status.success(), "executed stop");

    let cmd_output: StopCommandOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse stop output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_stop_provider_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "start",
            "provider",
            PROVIDER_HTTPSERVER_OCI_REF,
            "--output",
            "json",
            "--timeout-ms",
            "20000",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .output()
        .await
        .context("failed to start actor")?;
    assert!(output.status.success(), "executed start");

    let StartCommandOutput {
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

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "stop",
            "provider",
            &host_id,
            &provider_id,
            &link_name,
            &contract_id,
            "--output",
            "json",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .output()
        .await
        .context("failed to stop provider")?;

    assert!(output.status.success(), "executed stop");

    let cmd_output: StopCommandOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse stop output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_stop_host_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "stop",
            "host",
            &wash_instance.host_id,
            "--output",
            "json",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .output()
        .await
        .context("failed to stop provider")?;

    assert!(output.status.success(), "executed stop");

    let cmd_output: StopCommandOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}
