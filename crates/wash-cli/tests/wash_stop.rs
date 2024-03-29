mod common;

use common::{wait_for_no_hosts, TestWashInstance, HELLO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

use anyhow::{Context, Result};
use serial_test::serial;
use wash_lib::cli::output::StartCommandOutput;

#[tokio::test]
#[serial]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
async fn integration_stop_actor_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let StartCommandOutput {
        component_id,
        component_ref,
        host_id,
        success,
        ..
    } = wash_instance
        .start_actor(HELLO_OCI_REF, "hello_actor_id_from_start")
        .await?;
    assert!(success, "start command returned success");

    let actor_id = component_id.expect("missing actor_id from start command output");
    let actor_ref = component_ref.expect("missing actor_ref from start command output");
    assert_eq!(actor_ref, HELLO_OCI_REF, "actor ref matches");

    let host_id = host_id.expect("missing host_id from start command output");
    assert_eq!(host_id, wash_instance.host_id, "host_id matches");

    // Stop the actor
    wash_instance.stop_actor(&actor_id, Some(host_id)).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_stop_provider_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let StartCommandOutput {
        provider_id,
        provider_ref,
        host_id,
        success,
        ..
    } = wash_instance
        .start_provider(PROVIDER_HTTPSERVER_OCI_REF, "httpserver_stop")
        .await?;
    assert!(success, "start command returned success");

    let provider_ref = provider_ref.expect("missing provider_ref from start command output");
    assert_eq!(
        provider_ref, PROVIDER_HTTPSERVER_OCI_REF,
        "provider ref matches"
    );

    let provider_id = provider_id.expect("missing provider_id from start command output");
    let host_id = host_id.expect("missing host_id from start command output");
    assert_eq!(host_id, wash_instance.host_id, "host_id matches");

    wash_instance
        .stop_provider(&provider_id, Some(host_id))
        .await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_stop_host_serial() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;
    let wash_instance = TestWashInstance::create().await?;
    wash_instance.stop_host().await?;
    Ok(())
}
