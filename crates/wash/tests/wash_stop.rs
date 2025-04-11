mod common;

use common::{wait_for_no_hosts, TestWashInstance, HELLO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

use anyhow::{Context, Result};
use serial_test::serial;
use wash::lib::cli::output::StartCommandOutput;

#[tokio::test]
#[serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_stop_component_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let StartCommandOutput {
        component_id,
        component_ref,
        host_id,
        success,
        ..
    } = wash_instance
        .start_component(HELLO_OCI_REF, "hello_component_id_from_start")
        .await?;
    assert!(success, "start command returned success");

    let component_id = component_id.expect("missing component_id from start command output");
    let component_ref = component_ref.expect("missing component_ref from start command output");
    assert_eq!(component_ref, HELLO_OCI_REF, "component ref matches");

    let host_id = host_id.expect("missing host_id from start command output");
    assert_eq!(host_id, wash_instance.host_id, "host_id matches");

    // Stop the component
    wash_instance
        .stop_component(&component_id, Some(host_id))
        .await?;

    Ok(())
}

#[tokio::test]
#[serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
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
    let wash_instance = TestWashInstance::create_with_extra_args(["--disable-wadm"]).await?;
    wash_instance.stop_host().await?;
    Ok(())
}
