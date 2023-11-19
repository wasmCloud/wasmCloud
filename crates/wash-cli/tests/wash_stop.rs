mod common;

use common::{TestWashInstance, ECHO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

use anyhow::Result;
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
        actor_id,
        actor_ref,
        host_id,
        success,
        ..
    } = wash_instance.start_actor(ECHO_OCI_REF).await?;
    assert!(success, "start command returned success");

    let actor_id = actor_id.expect("missing actor_id from start command output");
    let actor_ref = actor_ref.expect("missing actor_ref from start command output");
    assert_eq!(actor_ref, ECHO_OCI_REF, "actor ref matches");

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
        link_name,
        contract_id,
        success,
        ..
    } = wash_instance
        .start_provider(PROVIDER_HTTPSERVER_OCI_REF)
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

    let link_name = link_name.expect("missing link_name from start command output");
    let contract_id = contract_id.expect("missing contract_id from start command output");

    wash_instance
        .stop_provider(&provider_id, &contract_id, Some(host_id), Some(link_name))
        .await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_stop_host_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;
    wash_instance.stop_host().await?;
    Ok(())
}
